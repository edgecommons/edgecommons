package <<PACKAGE>>;

import com.mbreissi.edgecommons.EdgeCommons;
import com.mbreissi.edgecommons.EdgeCommonsBuilder;
import com.mbreissi.edgecommons.EdgeCommonsInstance;
import com.mbreissi.edgecommons.commands.CommandInbox;
import com.mbreissi.edgecommons.config.ConfigManager;
import com.mbreissi.edgecommons.config.ConfigurationChangeListener;
import com.mbreissi.edgecommons.facades.DataFacade;
import com.mbreissi.edgecommons.facades.EventsFacade;
import com.mbreissi.edgecommons.facades.Severity;
import com.mbreissi.edgecommons.facades.SignalUpdate;
import com.mbreissi.edgecommons.heartbeat.InstanceConnectivity;
import com.mbreissi.edgecommons.metrics.MetricEmitter;
import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.google.gson.JsonPrimitive;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicLong;
import java.util.concurrent.locks.ReentrantLock;

/**
 * {@code <<COMPONENTNAME>>} — a southbound protocol adapter.
 *
 * <p>An <b>adapter</b> connects to devices, reads signals, and publishes them onto the UNS in the
 * shape the rest of the fleet expects — so that a consumer can chart a Modbus register and an OPC UA
 * node without knowing either protocol.
 *
 * <pre>
 *   connect -&gt; poll -&gt; publish SouthboundSignalUpdate -&gt; report health
 *      ^                                                        |
 *      +------------- reconnect with backoff &lt;-------------------+
 * </pre>
 *
 * <p>One worker per instance: an instance is one device, and its connection lifecycle is its own. That
 * worker owns the (single-threaded) device session; every command that must touch it or serialize with
 * the poll loop is routed through the worker's {@link Commands.DeviceControl} seam under a lock, and
 * <i>confirmed</i>. The command surface itself lives in {@link Commands}.
 *
 * <h2>The contract you are implementing (docs/SOUTHBOUND.md)</h2>
 * <ul>
 *   <li>Publish {@code SouthboundSignalUpdate} on the {@code data} class, <b>via the {@code data()}
 *       facade</b> — never hand-build the body and never hand-write the topic.</li>
 *   <li><b>Quality on every sample</b>, normalized to {@code GOOD | BAD | UNCERTAIN}, with the native
 *       code in {@code qualityRaw}.</li>
 *   <li>Emit <b>{@code southbound_health}</b> (the exact §5 set — see {@link Metrics}), dimensioned by
 *       instance, so an operator can see a link go down without reading logs.</li>
 *   <li>Report <b>per-instance connectivity</b> ({@link #connectivityOf}).</li>
 *   <li>Serve <b>read/write/browse/reconnect/pause commands</b> — and allow-list the writes.</li>
 * </ul>
 */
public class <<COMPONENTNAME>> implements ConfigurationChangeListener {

    private static final Logger LOGGER = LogManager.getLogger(<<COMPONENTNAME>>.class);

    /** How often the periodic metrics emit runs, in the poll loop (SOUTHBOUND.md §5). */
    private static final long METRICS_INTERVAL_MS = 30_000L;

    private final EdgeCommons edgeCommons;
    private final ConfigManager config;
    private final MetricEmitter metrics;
    private final List<DeviceConfig> devices = new ArrayList<>();
    private final long staleSignalSecs;

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
        metrics = edgeCommons.getMetrics();
        config.addConfigChangeListener(this);

        this.staleSignalSecs = Wiring.readStaleSignalSecs(config);

        for (String instanceId : config.getInstanceIds()) {
            try {
                devices.add(DeviceConfig.from(config.getInstanceConfig(instanceId)));
            } catch (RuntimeException e) {
                LOGGER.warn("skipping malformed device `{}`: {}", instanceId, e.toString());
            }
        }
        if (devices.isEmpty()) {
            throw new IllegalStateException("no valid devices in component.instances[]");
        }
    }

    public void run() {
        LOGGER.info("Starting adapter '{}' (thing={})", "<<COMPONENTFULLNAME>>", config.getThingName());

        // Each device's health, shared with its worker and read by the connectivity provider.
        List<Reported> reported = new ArrayList<>();
        // The per-device handles the command surface routes on.
        List<Commands.DeviceHandle> handles = new ArrayList<>();
        List<DeviceWorker> workers = new ArrayList<>();

        for (DeviceConfig device : devices) {
            // Per-instance facades: `data()` mints this device's topics and stamps its identity.
            EdgeCommonsInstance instance = edgeCommons.instance(device.id());

            Health health = new Health();
            DeviceMetrics dm = new DeviceMetrics(metrics, config, device.id(), health, staleSignalSecs);
            // Pre-define the metric set so it is fixed and discoverable at startup.
            dm.defineAll();

            Device.DeviceBackend backend = Device.backendFor(device.adapter());
            // The signal inventory `sb/signals` shows — a config/backend view, no device round-trip.
            List<Device.SignalInfo> signals =
                    backend != null ? backend.inventory(device.connection()) : List.of();

            DeviceWorker worker = new DeviceWorker(device, instance.data(), instance.events(), dm, health);
            reported.add(new Reported(device, health));
            handles.add(new Commands.DeviceHandle(device, worker, health, dm, signals));
            workers.add(worker);
        }

        // ONE provider, TWO surfaces: the library pushes this sample into the `state` keepalive's
        // instances[] every tick, and returns the very same sample from the built-in `status` verb.
        // Whoever watches and whoever asks cannot get different answers.
        edgeCommons.setInstanceConnectivityProvider(() -> {
            List<InstanceConnectivity> out = new ArrayList<>();
            for (Reported r : reported) {
                out.add(Wiring.connectivityOf(r.cfg(), r.health()));
            }
            return out;
        });

        // The southbound command surface (`Commands`). `ping` / `reload-config` / `get-configuration` /
        // `status` are already live — the library registered them before we ran.
        CommandInbox commands = edgeCommons.getCommands();
        if (commands != null) {
            Commands.registerAll(commands, handles);
        }

        // Start each device's worker (connect / poll / publish / reconnect).
        for (DeviceWorker worker : workers) {
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
        for (DeviceWorker worker : workers) {
            worker.stop();
        }
        metrics.flushMetrics();
        LOGGER.info("Adapter stopped");
    }

    @Override
    public boolean onConfigurationChanged() {
        LOGGER.info("Configuration changed");
        return true;
    }

    /** A configured device paired with its live health, for the connectivity provider. */
    private record Reported(DeviceConfig cfg, Health health) {
    }

    // =============================================================================================
    // The device worker: one device's lifecycle, and the control seam behind the sb/* verbs
    // =============================================================================================

    /**
     * One device's lifecycle: connect, poll, publish, reconnect — and the {@link Commands.DeviceControl}
     * seam the command surface routes on. The worker owns the (single-threaded) session; a
     * {@link ReentrantLock} serializes the poll loop with every session-touching command, so a write can
     * never race a read on the same connection — most device protocols are one request/response channel.
     *
     * <p><b>Java-idiom note:</b> the Rust reference gives each device a task plus a {@code mpsc} control
     * channel because its session is not {@code Sync}. Java threads + a per-device lock are the
     * idiomatic equivalent: the observable command contract (confirmed reads/writes, reconnect
     * drop+reestablish, repoll, pause gating) is identical.
     */
    final class DeviceWorker implements Commands.DeviceControl {

        private final DeviceConfig cfg;
        private final DataFacade data;
        private final EventsFacade events;
        private final DeviceMetrics dm;
        private final Health health;
        private final Device.DeviceBackend backend;
        private final Backoff backoff = new Backoff(1_000, 60_000);

        /** Guards {@link #session} and every session I/O — poll and commands never overlap. */
        private final ReentrantLock lock = new ReentrantLock();
        /** The live session, or null while down. Guarded by {@link #lock} for I/O. */
        private Device.DeviceSession session;

        private volatile boolean running = true;
        private Thread thread;
        private long lastMetricsEmit = System.nanoTime();

        DeviceWorker(DeviceConfig cfg, DataFacade data, EventsFacade events, DeviceMetrics dm,
                     Health health) {
            this.cfg = cfg;
            this.data = data;
            this.events = events;
            this.dm = dm;
            this.health = health;
            this.backend = Device.backendFor(cfg.adapter());
        }

        void start() {
            if (backend == null) {
                LOGGER.error("[{}] unknown adapter '{}' - worker not started", cfg.id(), cfg.adapter());
                return;
            }
            thread = new Thread(this::loop, "adapter-" + cfg.id());
            thread.setDaemon(true);
            thread.start();
        }

        void stop() {
            running = false;
            if (thread != null) {
                thread.interrupt();
            }
        }

        /** The supervisor loop: (re)connect with backoff, then poll until the link breaks. */
        private void loop() {
            int attempt = 0;
            while (running) {
                lock.lock();
                try {
                    if (session == null) {
                        session = tryConnect(attempt);
                    }
                } finally {
                    lock.unlock();
                }
                if (session == null) {
                    attempt++;
                    sleepMs(backoff.delayMs(attempt, rand01()));
                    continue;
                }
                attempt = 0;

                // Poll on the interval until the link breaks (or shutdown / a reconnect drops it).
                while (running && session != null) {
                    if (!health.isPaused()) {
                        lock.lock();
                        try {
                            if (session != null) {
                                pollOnce(session);
                            }
                        } catch (Device.DeviceException e) {
                            LOGGER.warn("[{}] read failed; reconnecting: {}", cfg.id(), e.toString());
                            health.incrementReadErrors();
                            dropSession();
                        } finally {
                            lock.unlock();
                        }
                    }
                    maybeEmitPeriodic();
                    if (session == null) {
                        break;
                    }
                    sleepMs(cfg.pollIntervalMs());
                }

                if (session == null && running) {
                    // The link dropped underneath us.
                    health.setLink(LinkState.BACKOFF);
                    health.incrementReconnects();
                    dm.onConnectionDropped(System.nanoTime());
                    dm.emitNow();
                    JsonObject ctx = new JsonObject();
                    ctx.addProperty("instance", cfg.id());
                    events.raiseAlarm("device-unreachable",
                            "lost the link to " + cfg.connection().endpoint(), ctx);
                }
            }
            closeSession();
        }

        /** One connect attempt, updating health/metrics/events. Returns the session, or null on failure. */
        private Device.DeviceSession tryConnect(int attempt) {
            dm.onConnectAttempt();
            health.setLink(attempt == 0 ? LinkState.CONNECTING : LinkState.BACKOFF);
            long now = System.nanoTime();
            try {
                Device.DeviceSession s = backend.connect(cfg.connection());
                dm.onConnected(now);
                health.setLink(LinkState.ONLINE);
                dm.emitNow();
                JsonObject ctx = new JsonObject();
                ctx.addProperty("instance", cfg.id());
                ctx.addProperty("adapter", backend.kind());
                events.emit(Severity.INFO, "device-connected",
                        "connected to " + cfg.connection().endpoint(), ctx);
                events.clearAlarm("device-unreachable", null);
                return s;
            } catch (Device.DeviceException e) {
                dm.onConnectFailure();
                health.setLink(LinkState.BACKOFF);
                LOGGER.warn("[{}] connect failed (permanent={}): {}", cfg.id(), !e.isTransient(),
                        e.toString());
                return null;
            }
        }

        /** One poll: read, publish each reading, record latencies + staleness. */
        private long pollOnce(Device.DeviceSession s) throws Device.DeviceException {
            long started = System.nanoTime();
            List<Device.Reading> readings = s.readSignals();
            health.setPollLatencyMs(msSince(started));

            long publishStarted = System.nanoTime();
            long published = 0;
            for (Device.Reading r : readings) {
                com.mbreissi.edgecommons.facades.Quality quality = switch (r.quality()) {
                    case GOOD -> com.mbreissi.edgecommons.facades.Quality.GOOD;
                    case BAD -> com.mbreissi.edgecommons.facades.Quality.BAD;
                    case UNCERTAIN -> com.mbreissi.edgecommons.facades.Quality.UNCERTAIN;
                };
                SignalUpdate.Sample sample =
                        new SignalUpdate.Sample(r.value(), quality, r.qualityRaw(), null, null);
                SignalUpdate.Builder b = data.signal(r.signalId());
                if (r.name() != null) {
                    b = b.name(r.name());
                }
                b.device(cfg.adapter(), cfg.id(), cfg.connection().endpoint()).addSample(sample);
                try {
                    b.publish();
                    published++;
                    // Feed the staleness tracker — a signal that keeps updating is not stale.
                    dm.onSignalUpdate(r.signalId(), System.nanoTime());
                } catch (RuntimeException e) {
                    LOGGER.warn("[{}] publish failed for {}: {}", cfg.id(), r.signalId(), e.toString());
                }
            }
            health.setPublishLatencyMs(msSince(publishStarted));
            return published;
        }

        private void maybeEmitPeriodic() {
            if (msSince(lastMetricsEmit) >= METRICS_INTERVAL_MS) {
                dm.emitPeriodic();
                lastMetricsEmit = System.nanoTime();
            }
        }

        /** Close and forget the current session (caller holds the lock). */
        private void dropSession() {
            if (session != null) {
                session.close();
                session = null;
            }
        }

        private void closeSession() {
            lock.lock();
            try {
                dropSession();
            } finally {
                lock.unlock();
            }
        }

        // ---- Commands.DeviceControl (session-touching verbs, serialized under the lock) -----------

        // The three session-only verbs are pure decision logic (a session-null guard + an
        // exception->error-code remap) over the Device.DeviceSession seam — no facade, no broker — so
        // that logic lives in the covered top-level Control class and is unit-tested there. Here the
        // worker only holds the lock (serializing with the poll loop) and hands the live session over.
        @Override
        public List<Device.Reading> readNow(List<String> ids)
                throws Commands.ReadFailedException, Commands.DeviceUnavailableException {
            lock.lock();
            try {
                return Control.readNow(session, ids);
            } finally {
                lock.unlock();
            }
        }

        @Override
        public void write(String signalId, JsonElement value)
                throws Commands.WriteFailedException, Commands.DeviceUnavailableException {
            lock.lock();
            try {
                Control.write(session, signalId, value);
            } finally {
                lock.unlock();
            }
        }

        @Override
        public Device.BrowsePage browse(String cursor, int max)
                throws Device.BrowseException, Commands.DeviceUnavailableException {
            lock.lock();
            try {
                return Control.browse(session, cursor, max);
            } finally {
                lock.unlock();
            }
        }

        @Override
        public boolean pause() {
            boolean changed = Wiring.setPaused(health, true);
            if (changed) {
                JsonObject ctx = new JsonObject();
                ctx.addProperty("instance", cfg.id());
                events.emit(Severity.WARNING, "adapter-paused", "telemetry production paused", ctx);
            }
            return changed;
        }

        @Override
        public boolean resume() {
            boolean changed = Wiring.setPaused(health, false);
            if (changed) {
                JsonObject ctx = new JsonObject();
                ctx.addProperty("instance", cfg.id());
                events.emit(Severity.INFO, "adapter-resumed", "telemetry production resumed", ctx);
            }
            return changed;
        }

        @Override
        public void reconnect() throws Commands.ReconnectFailedException,
                Commands.DeviceUnavailableException {
            lock.lock();
            try {
                dropSession();
                // One immediate, confirmed attempt. On failure the supervisor loop keeps retrying.
                dm.onConnectAttempt();
                long now = System.nanoTime();
                try {
                    Device.DeviceSession s = backend.connect(cfg.connection());
                    session = s;
                    dm.onConnected(now);
                    health.setLink(LinkState.ONLINE);
                    dm.emitNow();
                    events.clearAlarm("device-unreachable", null);
                } catch (Device.DeviceException e) {
                    dm.onConnectFailure();
                    health.setLink(LinkState.BACKOFF);
                    throw new Commands.ReconnectFailedException(e.getMessage());
                }
            } finally {
                lock.unlock();
            }
        }

        @Override
        public long repoll() throws Commands.DeviceUnavailableException {
            lock.lock();
            try {
                if (session == null) {
                    throw new Commands.DeviceUnavailableException("device is disconnected");
                }
                try {
                    return pollOnce(session);
                } catch (Device.DeviceException e) {
                    health.incrementReadErrors();
                    dropSession();
                    throw new Commands.DeviceUnavailableException("link error");
                }
            } finally {
                lock.unlock();
            }
        }
    }

    private static long msSince(long startedNanos) {
        return Math.max(0L, (System.nanoTime() - startedNanos) / 1_000_000L);
    }

    private static void sleepMs(long ms) {
        try {
            Thread.sleep(Math.max(0L, ms));
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        }
    }

    private static double rand01() {
        return Math.random();
    }
}

// =================================================================================================
// Shared per-device state (top-level package-private — used by Commands and Metrics too)
// =================================================================================================

/**
 * One device == one entry of {@code component.instances[]}.
 *
 * @param id             the instance id — the {@code {instance}} token of this device's UNS topics
 * @param adapter        which backend to use (matches {@link Device.DeviceBackend#kind()})
 * @param connection     how to reach the device
 * @param pollIntervalMs how often to read, in milliseconds
 * @param writes         the write allow-list (empty means read-only, the correct default)
 */
record DeviceConfig(String id, String adapter, Device.ConnectionConfig connection, long pollIntervalMs,
                    Writes writes) {

    static DeviceConfig from(JsonObject instance) {
        String id = instance.get("id").getAsString();
        String adapter = instance.has("adapter") && instance.get("adapter").isJsonPrimitive()
                ? instance.get("adapter").getAsString() : "sim";
        Device.ConnectionConfig connection = Device.ConnectionConfig.from(
                instance.has("connection") ? instance.getAsJsonObject("connection") : null);
        long pollIntervalMs = instance.has("pollIntervalMs") && instance.get("pollIntervalMs").isJsonPrimitive()
                ? instance.get("pollIntervalMs").getAsLong() : 5_000L;
        Writes writes = Writes.from(
                instance.has("writes") ? instance.getAsJsonObject("writes") : null);
        return new DeviceConfig(id, adapter, connection, pollIntervalMs, writes);
    }
}

/**
 * The write allow-list. Writes are permitted <b>only</b> by stable {@code signal.id}; an empty list
 * means the adapter is read-only, which is the correct default for anything touching a control system.
 */
record Writes(List<String> allow) {

    static Writes from(JsonObject writes) {
        List<String> allow = new ArrayList<>();
        if (writes != null && writes.has("allow") && writes.get("allow").isJsonArray()) {
            JsonArray a = writes.getAsJsonArray("allow");
            for (JsonElement e : a) {
                if (e.isJsonPrimitive()) {
                    allow.add(e.getAsString());
                }
            }
        }
        return new Writes(allow);
    }

    boolean permits(String signalId) {
        return allow.contains(signalId);
    }
}

/**
 * This adapter's <b>own vocabulary</b> for a link's condition — what it reports as
 * {@code InstanceConnectivity.state}. A boolean cannot tell "still trying" from "backing off after a
 * failure"; an operator needs to, so the richer token exists alongside the normalized flag.
 */
enum LinkState {
    /** Connecting for the first time; nothing has failed yet. */
    CONNECTING(0),
    /** The session is up and being polled. */
    ONLINE(1),
    /** The link failed; reconnecting with backoff. */
    BACKOFF(2);

    private final int code;

    LinkState(int code) {
        this.code = code;
    }

    int code() {
        return code;
    }

    String asString() {
        return name();
    }

    static LinkState fromCode(int code) {
        return switch (code) {
            case 1 -> ONLINE;
            case 2 -> BACKOFF;
            default -> CONNECTING;
        };
    }
}

/**
 * The shared per-device state the metrics emitter reads and the connectivity provider renders. The
 * gauges ({@code connectionState}, latencies) and the interval counters ({@code readErrors},
 * {@code reconnects}) feed {@code southbound_health} ({@link Metrics}); {@code paused} and {@code link}
 * feed the connectivity token and {@code sb/status}. One source, several surfaces — so a health dot, a
 * metric, and a status reply can never disagree.
 */
final class Health {

    private final AtomicLong connectionState = new AtomicLong();
    private final AtomicInteger link = new AtomicInteger(LinkState.CONNECTING.code());
    private final AtomicBoolean paused = new AtomicBoolean();
    private final AtomicLong pollLatencyMs = new AtomicLong();
    private final AtomicLong publishLatencyMs = new AtomicLong();
    private final AtomicLong readErrors = new AtomicLong();
    private final AtomicLong reconnects = new AtomicLong();

    /**
     * Record the link's condition. The metric's boolean and the reported state token move
     * <b>together</b>, so the health dot and the label a console shows can never disagree.
     */
    void setLink(LinkState state) {
        link.set(state.code());
        connectionState.set(state == LinkState.ONLINE ? 1 : 0);
    }

    LinkState link() {
        return LinkState.fromCode(link.get());
    }

    boolean isPaused() {
        return paused.get();
    }

    AtomicBoolean pausedFlag() {
        return paused;
    }

    long connectionState() {
        return connectionState.get();
    }

    long pollLatencyMs() {
        return pollLatencyMs.get();
    }

    long publishLatencyMs() {
        return publishLatencyMs.get();
    }

    void setPollLatencyMs(long v) {
        pollLatencyMs.set(v);
    }

    void setPublishLatencyMs(long v) {
        publishLatencyMs.set(v);
    }

    void incrementReadErrors() {
        readErrors.incrementAndGet();
    }

    void incrementReconnects() {
        reconnects.incrementAndGet();
    }

    /** Read-and-reset the read-error interval counter (the {@code southbound_health} emit convention). */
    long takeReadErrors() {
        return readErrors.getAndSet(0);
    }

    /** Read-and-reset the reconnect interval counter. */
    long takeReconnects() {
        return reconnects.getAndSet(0);
    }
}

/**
 * Reconnect backoff. Exponential with full jitter and a cap — so a site whose PLC reboots does not get
 * every adapter in the plant reconnecting in lockstep on the same second.
 */
record Backoff(long baseMs, long maxMs) {

    long delayMs(int attempt, double rand01) {
        long exp = baseMs << Math.min(attempt, 20);
        long cap = Math.min(exp, maxMs);
        double r = Math.max(0.0, Math.min(1.0, rand01));
        return (long) (r * cap);
    }
}

/**
 * The pure, broker-free wiring functions the component delegates to — the connectivity rendering, the
 * pause toggle, and the {@code staleSignalSecs} config read. The component class itself is a live
 * bootstrap + per-device worker loop (it needs a broker and a running {@code EdgeCommons} to connect
 * to anything) and is validated on real infrastructure, so it is excluded from the in-process
 * coverage gate; these functions need none of that, so they live here where a plain JUnit test covers
 * them directly.
 */
final class Wiring {

    private static final Logger LOGGER = LogManager.getLogger(Wiring.class);

    /** The {@code component.global.healthThresholds.staleSignalSecs} default (SOUTHBOUND.md §4/§5). */
    static final long DEFAULT_STALE_SIGNAL_SECS = 30L;

    private Wiring() {
    }

    /**
     * One device's connectivity sample, for the instance-connectivity provider.
     *
     * <ul>
     *   <li>{@code connected} is the <b>normalized</b> flag — always present.</li>
     *   <li>{@code state} is <i>this adapter's</i> vocabulary ({@link LinkState}) — {@code PAUSED} when
     *       paused and up, else the raw link token (so a break while paused still reads {@code BACKOFF},
     *       {@code connected} staying truthful).</li>
     *   <li>{@code attributes} is the <b>open</b> bag: domain data only this adapter understands.</li>
     * </ul>
     */
    static InstanceConnectivity connectivityOf(DeviceConfig cfg, Health health) {
        LinkState link = health.link();
        boolean connected = link == LinkState.ONLINE;
        boolean paused = health.isPaused();
        String state = paused && connected ? "PAUSED" : link.asString();

        Map<String, JsonElement> attributes = new LinkedHashMap<>();
        attributes.put("adapter", new JsonPrimitive(cfg.adapter()));
        attributes.put("paused", new JsonPrimitive(paused));

        return InstanceConnectivity.of(cfg.id(), connected, cfg.connection().endpoint())
                .withState(state)
                .withAttributes(attributes);
    }

    /**
     * Flip the paused flag, returning whether the state actually changed (idempotent — pausing an
     * already-paused device is not an error). The event is emitted by the caller.
     */
    static boolean setPaused(Health health, boolean paused) {
        return health.pausedFlag().getAndSet(paused) != paused;
    }

    /** Reads {@code component.global.healthThresholds.staleSignalSecs}, defaulting when unset/malformed. */
    static long readStaleSignalSecs(ConfigManager config) {
        try {
            JsonObject global = config.getGlobalConfig();
            if (global != null && global.has("healthThresholds")
                    && global.get("healthThresholds").isJsonObject()) {
                JsonObject h = global.getAsJsonObject("healthThresholds");
                if (h.has("staleSignalSecs") && h.get("staleSignalSecs").isJsonPrimitive()) {
                    return h.get("staleSignalSecs").getAsLong();
                }
            }
        } catch (RuntimeException e) {
            LOGGER.debug("staleSignalSecs lookup failed, defaulting: {}", e.toString());
        }
        return DEFAULT_STALE_SIGNAL_SECS;
    }
}

/**
 * The device-seam decision logic behind the session-only {@code sb/*} verbs — the session-null guard
 * and the per-verb exception-to-error-code remap for {@code sb/read}, {@code sb/write}, and
 * {@code sb/browse}. These verbs touch <b>only</b> the {@link Device.DeviceSession} seam (an interface
 * with no EdgeCommons imports), so their branching runs with no broker, no facade, and no live device
 * — which is why it is extracted here and unit-tested directly rather than hidden inside the excluded
 * worker.
 *
 * <p>The device worker's overrides do nothing but take the per-device lock (serializing the call with
 * the poll loop) and delegate here. The verbs that additionally <i>publish</i> or <i>emit</i> —
 * {@code reconnect} (a live {@code connect} + alarm-clear) and {@code repoll} (a live poll +
 * {@code data()} publish), plus {@code pause}/{@code resume} (a live event) — cannot run without a
 * live {@code EdgeCommons}, so their bodies stay in the worker and are validated by {@code LiveSimIT}
 * on real infrastructure. The pure part of pause/resume (the idempotent toggle) already lives in
 * {@link Wiring#setPaused}.
 */
final class Control {

    private Control() {
    }

    /**
     * {@code sb/read} — read the named ids now. A null session means the worker has no live link
     * ({@code DEVICE_UNAVAILABLE}); a link error mid-read is {@code READ_FAILED}.
     */
    static List<Device.Reading> readNow(Device.DeviceSession session, List<String> ids)
            throws Commands.ReadFailedException, Commands.DeviceUnavailableException {
        if (session == null) {
            throw Commands.DeviceUnavailableException.gone();
        }
        try {
            return session.readNamed(ids);
        } catch (Device.DeviceException e) {
            throw new Commands.ReadFailedException(e.getMessage());
        }
    }

    /**
     * {@code sb/write} — a confirmed, already-allow-listed write (the allow-list is enforced one layer
     * up, in {@link Commands}, before this is reached). A null session is {@code DEVICE_UNAVAILABLE}; a
     * device rejection is {@code WRITE_FAILED}.
     */
    static void write(Device.DeviceSession session, String signalId, JsonElement value)
            throws Commands.WriteFailedException, Commands.DeviceUnavailableException {
        if (session == null) {
            throw Commands.DeviceUnavailableException.gone();
        }
        try {
            session.writeSignal(signalId, value);
        } catch (Device.DeviceException e) {
            throw new Commands.WriteFailedException(e.getMessage());
        }
    }

    /**
     * {@code sb/browse} — one page of address-space discovery. A null session is reported as a browse
     * <i>failure</i> ({@code BROWSE_FAILED} — the link is down, which is not the same as the protocol
     * lacking discovery); a {@link Device.BrowseException} from the session passes straight through so
     * the command layer can still tell {@code BROWSE_UNSUPPORTED} from {@code BROWSE_FAILED}.
     */
    static Device.BrowsePage browse(Device.DeviceSession session, String cursor, int max)
            throws Device.BrowseException {
        if (session == null) {
            throw Device.BrowseException.failed("device is disconnected");
        }
        return session.browse(cursor, max);
    }
}
