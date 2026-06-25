package com.breissinger.javacomponentskeleton;

import com.breissinger.ggcommons.GGCommons;
import com.breissinger.ggcommons.GGCommonsBuilder;
import com.breissinger.ggcommons.config.ConfigManager;
import com.breissinger.ggcommons.config.ConfigurationChangeListener;
import com.breissinger.ggcommons.credentials.BasicAuth;
import com.breissinger.ggcommons.credentials.CredentialService;
import com.breissinger.ggcommons.credentials.PutOptions;
import com.breissinger.ggcommons.credentials.Secret;
import com.breissinger.ggcommons.messaging.MessagingClient;
import com.breissinger.ggcommons.metrics.MetricEmitter;
import com.breissinger.ggcommons.parameters.ParameterService;
import com.breissinger.ggcommons.messaging.Message;
import com.breissinger.ggcommons.messaging.MessageBuilder;
import com.breissinger.ggcommons.messaging.MessageHandler;
import com.breissinger.ggcommons.messaging.ReplyFuture;
import com.breissinger.ggcommons.metrics.Metric;
import com.breissinger.ggcommons.metrics.MetricBuilder;
import com.breissinger.ggcommons.streaming.StreamHandle;
import com.breissinger.ggcommons.streaming.StreamService;
import com.google.gson.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import software.amazon.awssdk.aws.greengrass.model.QOS;

import java.nio.charset.StandardCharsets;
import java.util.HashMap;
import java.util.Map;
import java.util.Optional;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.TimeUnit;


/**
 * Sample Java component demonstrating GGCommons library usage.
 * Shows configuration management, messaging patterns, metrics emission,
 * and proper resource cleanup using modern service-oriented architecture.
 */
public class App implements ConfigurationChangeListener
{
    private static final Logger LOGGER = LogManager.getLogger(App.class);

    private final ConfigManager configService;
    private final MessagingClient messagingService;
    private final MetricEmitter metricService;
    /** Initialized runtime, used to reach optional subsystems (e.g. credentials) at startup. */
    private final GGCommons ggCommons;
    /** Durable {@code telemetry} stream handle, or {@code null} if the config has no streaming section. */
    private final StreamHandle stream;
    /** Whether the IoT Core command subscription was established (so shutdown only unsubscribes it then). */
    private volatile boolean iotCoreSubscribed = false;

    private static final String PUB_TOPIC = "ggcommons/test/java/hello_world";
    private static final String REQ_TOPIC = "ggcommons/test/java/request";

    /** Config key (under {@code component.global}) naming the secret the component reads. */
    private static final String DEMO_SECRET_KEY = "demo_secret";
    /** Default secret name when {@code component.global.demo_secret} is absent. */
    private static final String DEFAULT_DEMO_SECRET = "skeleton/demo-secret";

    private volatile long publishInterval;
    private volatile boolean running = true;

    public static void main(String[] args) {
        App app = new App(args);
        
        // Add shutdown hook for graceful cleanup
        Runtime.getRuntime().addShutdownHook(new Thread(() -> {
            LOGGER.info("Shutting down component...");
            app.shutdown();
        }));
        
        app.run();
    }

    // Message handlers using modern MessageHandler interface
    private final MessageHandler ipcHelloWorldHandler;
    private final MessageHandler iotCoreHelloWorldHandler;
    private final MessageHandler requestHandler;

    private void publishRequest(String id, int waitTimeSecs) {
        LOGGER.info("Publishing request message {}", id);
        
        JsonObject requestPayload = new JsonObject();
        requestPayload.addProperty("id", id);
        requestPayload.addProperty("wait_time", waitTimeSecs);
        requestPayload.addProperty("timestamp", System.currentTimeMillis());
        
        Message request = MessageBuilder.create("RequestTest", "1.0")
            .withPayload(requestPayload)
            .withConfig(configService)
            .build();
        
        final ReplyFuture pending = messagingService.request(REQ_TOPIC, request);
        pending
            .orTimeout(10, TimeUnit.SECONDS)
            .thenAccept(reply -> {
                JsonObject replyBody = (JsonObject) reply.getBody();
                String originalId = replyBody.get("original_id").getAsString();
                long processingTime = replyBody.get("processing_time_ms").getAsLong();
                LOGGER.info("Received reply for {}: processed in {}ms", originalId, processingTime);

                // Emit latency metric
                Map<String, Float> metrics = new HashMap<>();
                metrics.put("replyLatency", (float) processingTime);
                metricService.emitMetric("performance", metrics);
            })
            .exceptionally(throwable -> {
                LOGGER.error("Request {} failed or timed out: {}", id, throwable.getMessage());
                // Release the reply subscription on timeout (no reply auto-unsubscribe path fires).
                messagingService.cancelRequest(pending);
                return null;
            });
    }



    @Override
    public boolean onConfigurationChanged() {
        try {
            LOGGER.info("Configuration changed. Reloading settings...");
            JsonObject globalConfig = configService.getGlobalConfig();
            
            if (globalConfig.has("publish_interval")) {
                long newInterval = globalConfig.get("publish_interval").getAsLong() * 1000L;
                if (newInterval != publishInterval) {
                    LOGGER.info("Publish interval changed from {}ms to {}ms", publishInterval, newInterval);
                    publishInterval = newInterval;
                }
            }
            
            LOGGER.info("Configuration reload completed successfully");
            return true;
        } catch (Exception e) {
            LOGGER.error("Failed to reload configuration", e);
            return false;
        }
    }

    private void defineMetrics() {
        // Define performance metrics for LOCAL broker
        Metric localPerformanceMetric = MetricBuilder.create("performance_local")
            .withConfig(configService)
            .addMeasure("replyLatency", "Milliseconds", 1)
            .addMeasure("messageCount", "Count", 1)
            .addMeasure("errorCount", "Count", 60)
            .addDimension("broker", "LOCAL")
            .build();
        metricService.defineMetric(localPerformanceMetric);
        
        // Define performance metrics for IOT_CORE broker
        Metric iotCorePerformanceMetric = MetricBuilder.create("performance_iotcore")
            .withConfig(configService)
            .addMeasure("replyLatency", "Milliseconds", 1)
            .addMeasure("messageCount", "Count", 1)
            .addMeasure("errorCount", "Count", 60)
            .addDimension("broker", "IOT_CORE")
            .build();
        metricService.defineMetric(iotCorePerformanceMetric);
        
        LOGGER.info("Metrics defined successfully");
    }

    public App(String[] args) {
        LOGGER.info("Initializing Java Component Skeleton...");
        
        // Initialize GGCommons with component name and arguments using builder
        ggCommons = GGCommonsBuilder.create("com.breissinger.greengrass.JavaComponentSkeleton")
                                              .withArgs(args)
                                              .build();

        // Get services through dependency injection
        configService = ggCommons.getConfigManager();
        messagingService = ggCommons.getMessaging();
        metricService = ggCommons.getMetrics();

        // Durable telemetry stream (null unless the config has a `streaming` section with a stream
        // named "telemetry"). The publish loop appends each message; the library's export engine
        // drains it to the configured sink (Kinesis) independently.
        StreamHandle telemetryStream = null;
        StreamService streamService = ggCommons.getStreams();
        if (streamService != null) {
            try {
                telemetryStream = streamService.stream("telemetry");
                LOGGER.info("Telemetry streaming enabled (stream 'telemetry')");
            } catch (Exception e) {
                LOGGER.warn("stream 'telemetry' unavailable; streaming disabled: {}", e.getMessage());
            }
        }
        stream = telemetryStream;

        // Initialize message handlers after services are available
        ipcHelloWorldHandler = (topic, message) -> {
            JsonObject body = (JsonObject) message.getBody();
            String id = body.get("id").getAsString();
            LOGGER.info("Received LOCAL message on topic {}: {}", topic, id);
        };

        iotCoreHelloWorldHandler = (topic, message) -> {
            JsonObject body = (JsonObject) message.getBody();
            String id = body.get("id").getAsString();
            LOGGER.info("Received IOT CORE message on topic {}: {}", topic, id);
        };

        requestHandler = (topic, msg) -> {
            JsonObject body = (JsonObject) msg.getBody();
            String id = body.get("id").getAsString();
            int waitTimeSecs = body.get("wait_time").getAsInt();
            
            LOGGER.info("Received request message [{}]: {}", topic, id);
            
            // Process request asynchronously to avoid blocking
            CompletableFuture.runAsync(() -> {
                try {
                    if (waitTimeSecs > 0) {
                        Thread.sleep(waitTimeSecs * 1000L);
                    }
                    
                    JsonObject replyPayload = new JsonObject();
                    replyPayload.addProperty("reply_message", "Request processed successfully");
                    replyPayload.addProperty("original_id", id);
                    replyPayload.addProperty("processing_time_ms", waitTimeSecs * 1000);
                    
                    Message reply = MessageBuilder.create("ReplyTest", "1.0")
                        .withPayload(replyPayload)
                        .withConfig(configService)
                        .build();
                    messagingService.reply(msg, reply);
                    
                    LOGGER.info("Published reply for request {}", id);
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                    LOGGER.error("Request processing interrupted for {}", id, e);
                } catch (Exception e) {
                    LOGGER.error("Error processing request {}", id, e);
                }
            });
        };
        
        // Initialize configuration
        initializeConfiguration();
        
        // Define metrics
        defineMetrics();
        
        // Set up messaging subscriptions
        setupSubscriptions();
        
        LOGGER.info("Component initialization completed");
    }
    
    private void initializeConfiguration() {
        configService.addConfigChangeListener(this);
        JsonObject globalConfig = configService.getGlobalConfig();
        publishInterval = globalConfig.has("publish_interval") ? 
            globalConfig.get("publish_interval").getAsLong() * 1000L : 5000L;
        LOGGER.info("Initial publish interval set to {}ms", publishInterval);
    }
    
    private void setupSubscriptions() {
        // Subscribe to request topic for request-reply pattern
        messagingService.subscribe(REQ_TOPIC, requestHandler, 1);
        LOGGER.info("Subscribed to request topic: {}", REQ_TOPIC);
        
        // Subscribe to hello world topic on both local and IoT Core. The IoT Core subscribe is
        // non-fatal: builds/modes without an IoT Core transport (e.g. local-only STANDALONE) skip
        // the bridge instead of failing component startup.
        messagingService.subscribe(PUB_TOPIC, ipcHelloWorldHandler, 3);
        try {
            messagingService.subscribeToIoTCore(PUB_TOPIC, iotCoreHelloWorldHandler, QOS.AT_LEAST_ONCE, 2);
            iotCoreSubscribed = true;
        } catch (Exception e) {
            LOGGER.warn("IoT Core unavailable; skipping IoT Core subscribe: {}", e.getMessage());
        }
        LOGGER.info("Subscribed to hello world topic: {}", PUB_TOPIC);
    }
    
    public void run() {
        LOGGER.info("Starting component execution...");

        // Demonstrate encrypted-vault secret access once at startup (non-fatal).
        demonstrateCredentials(ggCommons);

        // Demonstrate offline-first parameter access once at startup (non-fatal).
        demonstrateParameters(ggCommons);

        // Demonstrate request-reply pattern
        demonstrateRequestReply();
        
        // Main publish loop
        int messageId = 1;
        while (running) {
            try {
                publishHelloWorldMessage(messageId);
                measureRequestReplyLatency(messageId);
                
                messageId++;
                Thread.sleep(publishInterval);
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                LOGGER.info("Component execution interrupted");
                break;
            } catch (Exception e) {
                LOGGER.error("Error in main loop", e);
                emitErrorMetric();
            }
        }
        
        LOGGER.info("Component execution completed");
    }
    
    /**
     * Demonstrate encrypted-vault secret access via {@link GGCommons#getCredentials()}.
     *
     * <p>Shows the credential-service usage every real component needs: read a named secret from the
     * encrypted local vault and use it — without ever logging the value. Runs once at startup.
     *
     * <p>In production the secret arrives via central sync (AWS Secrets Manager over TES, with a
     * {@code credentials.central} config) or out-of-band provisioning; here, so the example is
     * self-contained, we seed a demo value locally on first run if it is absent.
     *
     * <p>Non-fatal: any vault error is logged and swallowed so the demo never takes the component down.
     */
    private void demonstrateCredentials(GGCommons gg) {
        try {
            CredentialService creds = gg.getCredentials();
            if (creds == null) {
                LOGGER.info("no credentials config section; secret access demo disabled");
                return;
            }

            JsonObject globalConfig = configService.getGlobalConfig();
            String name = globalConfig.has(DEMO_SECRET_KEY)
                ? globalConfig.get(DEMO_SECRET_KEY).getAsString()
                : DEFAULT_DEMO_SECRET;

            // Seed a demo secret on first run (in production this comes from central sync/provisioning).
            if (!creds.exists(name)) {
                JsonObject demo = new JsonObject();
                demo.addProperty("username", "svc-account");
                demo.addProperty("password", "demo-secret-value");
                byte[] bytes = demo.toString().getBytes(StandardCharsets.UTF_8);
                String version = creds.put(name, bytes, PutOptions.defaults());
                LOGGER.info("seeded demo secret (production: provided via central sync / provisioning) "
                    + "[secret={}, version={}]", name, version);
            }

            // Read it back and use it — logging only non-sensitive facts, never the value.
            Optional<Secret> secret = creds.get(name);
            if (secret.isPresent()) {
                Secret s = secret.get();
                LOGGER.info("credential access OK (value redacted) [secret={}, bytes={}, source={}]",
                    name, s.bytes().length, s.source());
                // A real component would now use the secret (e.g. authenticate a downstream client).
                // Demonstrate a typed view; log only the non-secret username.
                Optional<BasicAuth> basicAuth = creds.getBasicAuth(name);
                basicAuth.ifPresent(ba -> LOGGER.info(
                    "parsed basic-auth view (password redacted) [secret={}, username={}]", name, ba.username()));
            } else {
                LOGGER.warn("secret not found after seeding (unexpected) [secret={}]", name);
            }
        } catch (Exception e) {
            LOGGER.warn("secret access demo failed (non-fatal): {}", e.getMessage());
        }
    }

    /**
     * Demonstrate offline-first parameter access via {@link GGCommons#getParameters()}.
     *
     * <p>Mirrors {@link #demonstrateCredentials(GGCommons)} for configuration parameters: read a
     * couple of declared parameters from the cache (populated at startup from the configured source)
     * and use them. The example config wires the {@code env} source (no AWS, no provisioning), so the
     * values come from environment variables (e.g. {@code GG_PARAM_SKELETON_REGION=us-east-1},
     * {@code GG_PARAM_SKELETON_POOLSIZE=8}). Runs once at startup.
     *
     * <p>Only non-secret values are logged here; a real component must never log a value flagged
     * {@code secure}. Non-fatal: any parameter error is logged and swallowed so the demo never takes
     * the component down (offline-first — a missing/unreachable parameter is just empty).
     */
    private void demonstrateParameters(GGCommons gg) {
        try {
            ParameterService params = gg.getParameters();
            if (params == null) {
                LOGGER.info("no parameters config section; parameter access demo disabled");
                return;
            }

            // A plain string parameter (non-secret) — safe to log.
            Optional<String> region = params.get("/skeleton/region");
            region.ifPresentOrElse(
                r -> LOGGER.info("parameter access OK [param=/skeleton/region, value={}]", r),
                () -> LOGGER.info("parameter /skeleton/region not set (set GG_PARAM_SKELETON_REGION to populate it)"));

            // A typed (integer) parameter via getInt — non-secret tuning value, safe to log.
            Optional<Long> poolSize = params.getInt("/skeleton/poolSize");
            poolSize.ifPresentOrElse(
                p -> LOGGER.info("parameter access OK [param=/skeleton/poolSize, value={}]", p),
                () -> LOGGER.info("parameter /skeleton/poolSize not set (set GG_PARAM_SKELETON_POOLSIZE to populate it)"));

            LOGGER.info("parameter subsystem stats [{}]", params.stats());
        } catch (Exception e) {
            LOGGER.warn("parameter access demo failed (non-fatal): {}", e.getMessage());
        }
    }

    private void demonstrateRequestReply() {
        LOGGER.info("Demonstrating request-reply pattern...");
        publishRequest("demo_1", 0);
        publishRequest("demo_2", 1);
        publishRequest("demo_3", 2);
        
        // Allow time for async requests to complete
        try {
            Thread.sleep(3000);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        }
    }
    
    private void publishHelloWorldMessage(int messageId) {
        JsonObject payload = new JsonObject();
        payload.addProperty("id", messageId);
        payload.addProperty("message", "Hello World from Java Component");
        payload.addProperty("timestamp", System.currentTimeMillis());
        payload.addProperty("component", "JavaComponentSkeleton");
        
        Message msg = MessageBuilder.create("HelloWorld", "1.0")
            .withPayload(payload)
            .withConfig(configService)
            .build();
        
        // Publish to both local and IoT Core to demonstrate dual connectivity (IoT Core non-fatal).
        messagingService.publish(PUB_TOPIC, msg);
        try {
            messagingService.publishToIotCore(PUB_TOPIC, msg, QOS.AT_LEAST_ONCE);
        } catch (Exception e) {
            LOGGER.warn("failed to publish to IoT Core: {}", e.getMessage());
        }

        // Append the data point to the durable telemetry stream (partitioned by Thing). Append
        // returns once committed to the local buffer; the export engine drains it to the sink.
        if (stream != null) {
            try {
                String thing = configService.getThingName();
                JsonObject streamPayload = new JsonObject();
                streamPayload.addProperty("id", messageId);
                streamPayload.addProperty("thing", thing);
                stream.append(thing, System.currentTimeMillis(),
                              streamPayload.toString().getBytes(StandardCharsets.UTF_8));
            } catch (Exception e) {
                LOGGER.warn("failed to append to telemetry stream: {}", e.getMessage());
            }
        }

        LOGGER.debug("Published hello world message {} to both local and IoT Core", messageId);
    }
    
    private void measureRequestReplyLatency(int messageId) {
        // Measure LOCAL broker latency
        measureLatency("latency_test_local_" + messageId, "LOCAL");

        // Measure IOT_CORE broker latency only when IoT Core is available (skipped in
        // local-only STANDALONE; otherwise requestFromIoTCore throws synchronously and would
        // bubble to the main loop before the interval sleep, busy-spinning the publisher).
        if (iotCoreSubscribed) {
            measureLatency("latency_test_iotcore_" + messageId, "IOT_CORE");
        }
    }
    
    private void measureLatency(String requestId, String brokerType) {
        long startTime = System.currentTimeMillis();
        
        JsonObject requestPayload = new JsonObject();
        requestPayload.addProperty("id", requestId);
        requestPayload.addProperty("wait_time", 0); // No artificial delay
        requestPayload.addProperty("timestamp", startTime);
        requestPayload.addProperty("broker_type", brokerType);
        
        Message request = MessageBuilder.create("LatencyTest", "1.0")
            .withPayload(requestPayload)
            .withConfig(configService)
            .build();
        
        // Use different request methods for different brokers. Guard against a synchronous
        // throw (e.g. IoT Core not connected) so it never escapes to the main publish loop.
        ReplyFuture requestFuture;
        try {
            if ("LOCAL".equals(brokerType)) {
                requestFuture = messagingService.request(REQ_TOPIC, request);
            } else {
                requestFuture = messagingService.requestFromIoTCore(REQ_TOPIC, request);
            }
        } catch (Exception e) {
            LOGGER.warn("latency request dispatch failed for {} broker: {}", brokerType, e.getMessage());
            return;
        }

        // Effectively-final handle so the timeout path below can release the reply subscription.
        final ReplyFuture pending = requestFuture;
        requestFuture
            .orTimeout(5, TimeUnit.SECONDS)
            .thenAccept(reply -> {
                long endTime = System.currentTimeMillis();
                long latency = endTime - startTime;
                
                // Emit latency metric for specific broker type
                Map<String, Float> metrics = new HashMap<>();
                metrics.put("replyLatency", (float) latency);
                metrics.put("messageCount", 1.0f);
                
                String metricName = "LOCAL".equals(brokerType) ? "performance_local" : "performance_iotcore";
                metricService.emitMetric(metricName, metrics);
                
                LOGGER.debug("Measured {} latency: {}ms for request {}", brokerType, latency, requestId);
            })
            .exceptionally(throwable -> {
                LOGGER.warn("Latency measurement failed for {} broker, request {}: {}",
                           brokerType, requestId, throwable.getMessage());
                // No reply arrived (e.g. IoT Core not connected): the library only auto-unsubscribes
                // the reply topic on a *received* reply, so a timed-out request must be cancelled
                // explicitly. Without this the orphaned ggcommons/reply-<uuid> subscription (and its
                // pending-future entry) accumulate every cycle and eventually exhaust the IPC
                // subscription quota. Mirrors the Python skeleton's cancel-on-timeout.
                if ("LOCAL".equals(brokerType)) {
                    messagingService.cancelRequest(pending);
                } else {
                    messagingService.cancelRequestFromIoTCore(pending);
                }
                emitErrorMetric();
                return null;
            });
    }
    
    private void emitErrorMetric() {
        Map<String, Float> metrics = new HashMap<>();
        metrics.put("errorCount", 1.0f);
        metricService.emitMetric("performance_local", metrics);
    }
    
    public void shutdown() {
        LOGGER.info("Shutting down component...");
        running = false;
        
        try {
            // Unsubscribe from topics (only unsubscribe IoT Core if we subscribed).
            messagingService.unsubscribe(PUB_TOPIC);
            messagingService.unsubscribe(REQ_TOPIC);
            if (iotCoreSubscribed) {
                messagingService.unsubscribeFromIoTCore(PUB_TOPIC);
            }
            LOGGER.info("Unsubscribed from all topics");
        } catch (Exception e) {
            LOGGER.error("Error during shutdown", e);
        }
        
        LOGGER.info("Component shutdown completed");
    }
}