package <<PACKAGE>>;

import com.mbreissi.edgecommons.EdgeCommons;
import com.mbreissi.edgecommons.EdgeCommonsBuilder;
import com.mbreissi.edgecommons.EdgeCommonsInstance;
import com.mbreissi.edgecommons.config.ConfigManager;
import com.mbreissi.edgecommons.config.ConfigurationChangeListener;
import com.mbreissi.edgecommons.heartbeat.InstanceConnectivity;
import com.mbreissi.edgecommons.messaging.Message;
import com.mbreissi.edgecommons.messaging.MessagingClient;
import com.mbreissi.edgecommons.metrics.Metric;
import com.mbreissi.edgecommons.metrics.MetricBuilder;
import com.mbreissi.edgecommons.metrics.MetricEmitter;
import com.mbreissi.edgecommons.uns.UnsClass;
import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import com.google.gson.JsonPrimitive;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.time.Instant;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
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
 * by default) on {@code ecv1/{device}/{component}/main/state} — no heartbeat code here. What the
 * adapter <i>does</i> supply is the payload only it knows: <b>one connectivity entry per configured
 * device</b> ({@link #reportConnectivity}), which the library both pushes on that keepalive and
 * returns from the built-in {@code status} verb.
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

    /**
     * The live reachability of every configured device, keyed by instance id — the adapter's answer
     * to "which of my devices are actually up?". Written by the instance workers, read on the
     * heartbeat thread; hence concurrent, and hence a cached value rather than a live probe (see
     * {@link #reportConnectivity}).
     */
    private final Map<String, InstanceConnectivity> deviceHealth = new ConcurrentHashMap<>();

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

        // ONE provider, TWO surfaces: the library pushes this sample into the `state` keepalive's
        // instances[] every tick AND returns it from the built-in `status` verb when pulled — so an
        // operator asking "which devices are up?" and a console subscribed to state can never
        // disagree. Sampled on the heartbeat thread: it must not block, so it reads the cached map
        // the workers maintain and never touches the protocol.
        edgeCommons.setInstanceConnectivityProvider(() -> List.copyOf(deviceHealth.values()));
    }

    public void run() {
        LOGGER.info("Starting adapter '{}' (thing={}, UNS identity path={})",
                "<<COMPONENTFULLNAME>>", config.getThingName(),
                edgeCommons.getUns().identity().getPath());

        // One worker per configured instance (component.instances[].id). Each instance is one
        // device/endpoint with its own connection + subscriptions (see the southbound config convention).
        for (String instanceId : config.getInstanceIds()) {
            // Report the device BEFORE its worker connects: a configured device that is not yet up
            // must still appear (connected=false), or an operator cannot tell "still connecting"
            // from "not configured at all".
            reportConnectivity(instanceId, "", false, "CONNECTING", null);
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
            reportConnectivity(instanceId, endpoint, true, "ONLINE", null);

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
            // The device stays in the report — as DOWN. Removing it would make a broken device
            // indistinguishable from one nobody configured.
            reportConnectivity(instanceId, endpoint, false, "BACKOFF", e.toString());
        }
    }

    /**
     * Records one device's live reachability for both connectivity surfaces (the {@code state}
     * keepalive's {@code instances[]} and the {@code status} verb). Call it wherever this adapter
     * already <i>knows</i> — connected, connection lost, retrying, administratively disabled.
     *
     * <p>{@code connected} is the one <b>normalized</b> field and is always present, so any console
     * can render a health dot for any adapter without knowing this protocol. {@code state} is this
     * adapter's <i>own</i> vocabulary for what a boolean cannot say — {@code CONNECTING} and
     * {@code BACKOFF} are both "not connected", but only one of them means an operator should look
     * at the device. {@code attributes} is an open bag for protocol-specific facts (a session id,
     * a firmware revision, the last error code): it is deliberately unconstrained, so what only this
     * adapter understands can never destabilize the fields every consumer relies on.
     *
     * @param instanceId the device's {@code component.instances[].id}
     * @param endpoint   the device endpoint — the human {@code detail} when up
     * @param connected  the normalized reachability flag
     * @param state      this adapter's own condition token ({@code CONNECTING}/{@code ONLINE}/…)
     * @param detail     why it is down, or {@code null} to use the endpoint
     */
    private void reportConnectivity(String instanceId, String endpoint, boolean connected,
                                    String state, String detail) {
        deviceHealth.put(instanceId, connectivity(instanceId, endpoint, connected, state, detail));
    }

    /**
     * One device's connectivity entry — the pure half of {@link #reportConnectivity}, so the
     * adapter's report can be asserted without a device or a live runtime.
     *
     * @param instanceId the device's {@code component.instances[].id}
     * @param endpoint   the device endpoint — the human {@code detail} when up
     * @param connected  the normalized reachability flag
     * @param state      this adapter's own condition token ({@code CONNECTING}/{@code ONLINE}/…)
     * @param detail     why it is down, or {@code null} to use the endpoint
     * @return the entry the {@code state} keepalive and the {@code status} verb both report
     */
    static InstanceConnectivity connectivity(String instanceId, String endpoint, boolean connected,
                                             String state, String detail) {
        return InstanceConnectivity.of(instanceId, connected, detail != null ? detail : endpoint)
                .withState(state)
                .withAttributes(Map.of("adapter", new JsonPrimitive(ADAPTER_KIND)));
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
