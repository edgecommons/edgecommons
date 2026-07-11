package <<PACKAGE>>;

import com.mbreissi.edgecommons.EdgeCommons;
import com.mbreissi.edgecommons.EdgeCommonsBuilder;
import com.mbreissi.edgecommons.EdgeCommonsInstance;
import com.mbreissi.edgecommons.config.ConfigManager;
import com.mbreissi.edgecommons.config.ConfigurationChangeListener;
import com.mbreissi.edgecommons.messaging.Message;
import com.mbreissi.edgecommons.messaging.MessagingClient;
import com.mbreissi.edgecommons.metrics.Metric;
import com.mbreissi.edgecommons.metrics.MetricBuilder;
import com.mbreissi.edgecommons.metrics.MetricEmitter;
import com.mbreissi.edgecommons.uns.UnsClass;
import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.time.Instant;
import java.util.HashMap;
import java.util.Map;
import java.util.concurrent.CountDownLatch;

/**
 * Protocol-adapter scaffold built on the EdgeCommons Java library.
 *
 * <p>This is a <b>southbound adapter</b>: it talks to field devices/servers over some protocol
 * (OPC UA, Modbus, EtherNet/IP, …) and republishes their values northbound on the EdgeCommons
 * messaging bus using the standard <b>southbound contract</b> (see {@code docs/SOUTHBOUND.md}):
 * the {@code SouthboundSignalUpdate} envelope and the {@code southbound_health} metric.
 *
 * <p><b>UNS data plane</b>: each signal update is published on a UNS {@code data} topic minted
 * per device instance — {@code ecv1/{device}/{component}/{instanceId}/data/{signalPath}} via
 * {@code gg.instance(instanceId).uns().topic(UnsClass.DATA, signalPath)} — and its envelope is
 * identity-stamped via {@code gg.instance(instanceId).newMessage(...)}. Identity is config-driven
 * (top-level {@code hierarchy} + {@code identity} blocks; the last hierarchy level is always the
 * resolved thing name). Never hand-write topic strings. Consumers subscribe to
 * {@code ecv1/+/+/+/data/#}.
 *
 * <p>The {@code state} heartbeat keepalive is <b>automatic</b> (library-owned, on / 5 s / local
 * by default) on {@code ecv1/{device}/{component}/main/state} — no heartbeat code here.
 *
 * <p><b>Phase 5 (M9) note</b>: the southbound <i>command</i> family — write/read/control toward
 * the device — is not part of this scaffold yet. When you add such handlers, expect them to be
 * reworked onto UNS {@code cmd/sb/*} verbs on the component inbox
 * ({@code ecv1/{device}/{component}/{instance}/cmd/sb/write} etc.) when the library's Phase-5
 * command facade lands; keep them isolated so the retarget stays mechanical.
 *
 * <p>The framework gives you config, messaging, metrics, credentials, and lifecycle for free —
 * you write only the protocol code where the {@code TODO(adapter)} markers are.
 */
public class <<COMPONENTNAME>> implements ConfigurationChangeListener {

    private static final Logger LOGGER = LogManager.getLogger(<<COMPONENTNAME>>.class);

    /** Standard southbound message name + version (the contract; do not rename). */
    private static final String SOUTHBOUND_MSG = "SouthboundSignalUpdate";
    private static final String SOUTHBOUND_VER = "1.0";
    /** Standard adapter health metric (the contract). */
    private static final String HEALTH_METRIC = "southbound_health";
    /** Protocol identifier emitted in body.device.adapter — set to your protocol. */
    private static final String ADAPTER_KIND = "example";

    private final EdgeCommons edgeCommons;
    private final ConfigManager config;
    private final MessagingClient messaging;
    private final MetricEmitter metrics;

    /** Blocks main() until the JVM is signalled; the library's SIGTERM/SIGINT hook drives shutdown. */
    private final CountDownLatch shutdownLatch = new CountDownLatch(1);

    public static void main(String[] args) {
        // No manual shutdown hook: the EdgeCommons library wires SIGTERM/SIGINT to its graceful,
        // idempotent shutdown() (flips /readyz to 503, unsubscribes, closes messaging/metrics/…).
        new <<COMPONENTNAME>>(args).run();
    }

    public <<COMPONENTNAME>>(String[] args) {
        edgeCommons = EdgeCommonsBuilder.create("<<COMPONENTFULLNAME>>")
                .withArgs(args)
                .initialReady(false)
                .build();
        config = edgeCommons.getConfigManager();
        messaging = edgeCommons.getMessaging();
        metrics = edgeCommons.getMetrics();
        config.addConfigChangeListener(this);
        defineHealthMetric();
    }

    public void run() {
        LOGGER.info("Starting adapter '{}' (thing={}, UNS identity path={})",
                "<<COMPONENTFULLNAME>>", config.getThingName(),
                edgeCommons.getUns().identity().getPath());

        // One worker per configured instance (component.instances[].id). Each instance is one
        // device/endpoint with its own connection + subscriptions (see the southbound config convention).
        for (String instanceId : config.getInstanceIds()) {
            Thread worker = new Thread(() -> runInstance(instanceId), "adapter-" + instanceId);
            worker.setDaemon(true);
            worker.start();
        }

        // Required workers have been launched. Messaging connectivity and the command-inbox
        // acknowledgement remain mandatory parts of the runtime readiness predicate.
        edgeCommons.setReady(true);

        // Block until shutdown. The library's signal hook closes everything and the JVM exits 0.
        try {
            shutdownLatch.await();
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        }
        LOGGER.info("Adapter stopped");
    }

    /** Drives a single device instance: connect, subscribe/poll, publish updates. */
    private void runInstance(String instanceId) {
        JsonObject instance = config.getInstanceConfig(instanceId);
        JsonObject connection = instance.has("connection") ? instance.getAsJsonObject("connection") : new JsonObject();
        String endpoint = connection.has("endpoint") ? connection.get("endpoint").getAsString() : "";
        LOGGER.info("[{}] connecting to {}", instanceId, endpoint);

        try {
            // TODO(adapter): open the protocol connection to `endpoint` (with retry/backoff), then
            // establish subscriptions / start polling per instance.subscriptions[]. On each value
            // received, call publishUpdate(...). Emit connectionState=1 once connected.
            emitHealth(instanceId, /*connected*/ true, /*pollLatencyMs*/ 0, /*readErrors*/ 0, /*staleSignals*/ 0);

            // --- placeholder so the scaffold runs end-to-end; replace with real device events ---
            while (true) {
                JsonObject address = new JsonObject();          // protocol-native identity (opaque to consumers)
                address.addProperty("example", "sensor-1");
                publishUpdate(instanceId, endpoint, "example/sensor-1", "Sensor 1", address,
                              42.0, "GOOD", "Good", Instant.now().toString());
                Thread.sleep(1000);
            }
            // --- end placeholder ---
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        } catch (Exception e) {
            LOGGER.error("[{}] adapter error", instanceId, e);
            emitHealth(instanceId, false, 0, 1, 0);
        }
    }

    /**
     * Publish one signal update using the standard SouthboundSignalUpdate envelope (docs/SOUTHBOUND.md §2).
     * Quality is normalized to GOOD|BAD|UNCERTAIN with the native code retained in qualityRaw.
     */
    private void publishUpdate(String instanceId, String endpoint, String signalId, String signalName,
                               JsonObject address, Object value, String quality, String qualityRaw, String sourceTs) {
        JsonObject device = new JsonObject();
        device.addProperty("adapter", ADAPTER_KIND);
        device.addProperty("instance", instanceId);
        device.addProperty("endpoint", endpoint);

        JsonObject signal = new JsonObject();
        signal.addProperty("id", signalId);
        signal.addProperty("name", signalName);
        signal.add("address", address);

        JsonObject sample = new JsonObject();
        sample.addProperty("value", String.valueOf(value));   // TODO(adapter): preserve native JSON type
        sample.addProperty("quality", quality);
        sample.addProperty("qualityRaw", qualityRaw);
        sample.addProperty("sourceTs", sourceTs);
        JsonArray samples = new JsonArray();
        samples.add(sample);

        JsonObject body = new JsonObject();
        body.add("device", device);
        body.add("signal", signal);
        body.add("samples", samples);

        // The instance handle pre-binds the component.instances[].id token into both the topic
        // builder and the message builder, so topic and envelope carry the same identity.
        EdgeCommonsInstance instance = edgeCommons.instance(instanceId);

        // newMessage(...) stamps the envelope's `identity` block (hierarchy + device + component
        // + this instance) automatically from config — no manual thing/tag wiring.
        Message msg = instance.newMessage(SOUTHBOUND_MSG, SOUTHBOUND_VER)
                .withPayload(body)
                .build();

        // UNS data topic: ecv1/{device}/{component}/{instanceId}/data/{signalPath}. The signal id
        // is used directly as the channel path here; its tokens must satisfy the UNS token rule
        // (no '/'-traversal, '+', '#', '\', control chars — the config sanitizer's blacklist).
        // Phase 5 (D-U15) will formalize sanitized data/{channel} with the raw id in the body.
        String topic = instance.uns().topic(UnsClass.DATA, signalId);
        messaging.publish(topic, msg);
    }

    private void defineHealthMetric() {
        Metric health = MetricBuilder.create(HEALTH_METRIC)
                .withConfig(config)
                .addMeasure("connectionState", "Count", 1)
                .addMeasure("publishLatencyMs", "Milliseconds", 1)
                .addMeasure("pollLatencyMs", "Milliseconds", 1)
                .addMeasure("readErrors", "Count", 60)
                .addMeasure("staleSignals", "Count", 60)
                .build();
        metrics.defineMetric(health);
    }

    private void emitHealth(String instanceId, boolean connected, long pollLatencyMs, int readErrors, int staleSignals) {
        Map<String, Float> m = new HashMap<>();
        m.put("connectionState", connected ? 1.0f : 0.0f);
        m.put("pollLatencyMs", (float) pollLatencyMs);
        m.put("readErrors", (float) readErrors);
        m.put("staleSignals", (float) staleSignals);
        metrics.emitMetric(HEALTH_METRIC, m);
    }

    @Override
    public boolean onConfigurationChanged() {
        // TODO(adapter): re-read instance/subscription config and apply (e.g. add/remove subscriptions).
        LOGGER.info("Configuration changed");
        return true;
    }
}
