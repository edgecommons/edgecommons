package com.aws.proserve.javacomponentskeleton;

import com.aws.proserve.ggcommons.GGCommons;
import com.aws.proserve.ggcommons.GGCommonsBuilder;
import com.aws.proserve.ggcommons.config.ConfigManager;
import com.aws.proserve.ggcommons.config.ConfigurationChangeListener;
import com.aws.proserve.ggcommons.messaging.MessagingClient;
import com.aws.proserve.ggcommons.metrics.MetricEmitter;
import com.aws.proserve.ggcommons.messaging.Message;
import com.aws.proserve.ggcommons.messaging.MessageBuilder;
import com.aws.proserve.ggcommons.messaging.MessageHandler;
import com.aws.proserve.ggcommons.metrics.Metric;
import com.aws.proserve.ggcommons.metrics.MetricBuilder;
import com.google.gson.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import software.amazon.awssdk.aws.greengrass.model.QOS;

import java.util.HashMap;
import java.util.Map;
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
    
    private static final String PUB_TOPIC = "ggcommons/test/java/hello_world";
    private static final String REQ_TOPIC = "ggcommons/test/java/request";

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
        
        messagingService.request(REQ_TOPIC, request)
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
        GGCommons ggCommons = GGCommonsBuilder.create("aws.proserve.greengrass.JavaComponentSkeleton")
                                              .withArgs(args)
                                              .build();
        
        // Get services through dependency injection
        configService = ggCommons.getConfigManager();
        messagingService = ggCommons.getMessaging();
        metricService = ggCommons.getMetrics();
        
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
        
        // Subscribe to hello world topic on both local and IoT Core
        messagingService.subscribe(PUB_TOPIC, ipcHelloWorldHandler, 3);
        messagingService.subscribeToIoTCore(PUB_TOPIC, iotCoreHelloWorldHandler, QOS.AT_LEAST_ONCE, 2);
        LOGGER.info("Subscribed to hello world topic: {}", PUB_TOPIC);
    }
    
    public void run() {
        LOGGER.info("Starting component execution...");
        
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
        
        // Publish to both local and IoT Core to demonstrate dual connectivity
        messagingService.publish(PUB_TOPIC, msg);
        messagingService.publishToIotCore(PUB_TOPIC, msg, QOS.AT_LEAST_ONCE);
        
        LOGGER.debug("Published hello world message {} to both local and IoT Core", messageId);
    }
    
    private void measureRequestReplyLatency(int messageId) {
        // Measure LOCAL broker latency
        measureLatency("latency_test_local_" + messageId, "LOCAL");
        
        // Measure IOT_CORE broker latency  
        measureLatency("latency_test_iotcore_" + messageId, "IOT_CORE");
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
        
        // Use different request methods for different brokers
        CompletableFuture<Message> requestFuture;
        if ("LOCAL".equals(brokerType)) {
            requestFuture = messagingService.request(REQ_TOPIC, request);
        } else {
            requestFuture = messagingService.requestFromIoTCore(REQ_TOPIC, request);
        }
        
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
            // Unsubscribe from topics
            messagingService.unsubscribe(PUB_TOPIC);
            messagingService.unsubscribe(REQ_TOPIC);
            messagingService.unsubscribeFromIoTCore(PUB_TOPIC);
            LOGGER.info("Unsubscribed from all topics");
        } catch (Exception e) {
            LOGGER.error("Error during shutdown", e);
        }
        
        LOGGER.info("Component shutdown completed");
    }
}