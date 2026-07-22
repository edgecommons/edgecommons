package <<PACKAGE>>;

import com.mbreissi.edgecommons.config.ConfigManager;
import com.mbreissi.edgecommons.metrics.MetricBuilder;
import com.mbreissi.edgecommons.metrics.MetricEmitter;
import com.google.gson.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * Operational metrics — the canonical {@code southbound_health} + the operational-family pattern.
 *
 * <p>Every southbound adapter emits {@link #HEALTH} with <b>exactly</b> the SOUTHBOUND.md §5 measure
 * set. On top of that, this module ships the <b>operational-family pattern</b> two protocols deep as
 * worked examples — {@link #CONNECTION} and {@link #COMMAND} — and shows you where to add your own.
 *
 * <h2>What {@code <<COMPONENTNAME>>} emits today</h2>
 * <table border="1">
 *   <caption>metric families</caption>
 *   <tr><th>Metric</th><th>Dimensions</th><th>What it is</th></tr>
 *   <tr><td>{@code southbound_health}</td><td>{@code instance}</td>
 *       <td>the §5 canonical set (below) — every adapter emits this</td></tr>
 *   <tr><td>{@code <<COMPONENTNAME>>Connection}</td><td>{@code instance}</td>
 *       <td>the connect/reconnect lifecycle</td></tr>
 *   <tr><td>{@code <<COMPONENTNAME>>Command}</td><td>{@code instance}, {@code verb}, {@code result}</td>
 *       <td>the {@code sb/*} command surface</td></tr>
 * </table>
 *
 * <h2>The Total/Interval counter convention</h2>
 * Every <b>counter</b> is emitted as a measure PAIR: {@code <name>Total} (monotonic since start) and
 * {@code <name>Interval} (since the previous emit of that family; <b>reset on emit</b> — see
 * {@link Pair}). <b>Gauges</b> ({@code connectionState}) and interval <b>sums</b> (the {@code *Ms}
 * latencies/durations) are single measures. This is the same convention {@code modbus-adapter} and
 * {@code ethernet-ip-adapter} use, so a fleet dashboard reads every adapter the same way.
 *
 * <h2>Dimensions are LOW-CARDINALITY only</h2>
 * {@code instance}, {@code verb} (the closed {@link #COMMAND_VERBS} set), and {@code result}
 * ({@code success}|{@code error}) — and nothing else. <b>Never</b> dimension by signal name, address,
 * endpoint, or error text: those are unbounded and would shred a fleet dashboard.
 *
 * <h2>Add your protocol's families HERE</h2>
 * {@code <<COMPONENTNAME>>Connection}/{@code Command} are generic — every adapter has them. Your
 * protocol also has an <b>inventory</b> (configured signals), a <b>poll/subscribe</b> path, and a
 * <b>publish</b> path worth measuring. Add {@code <<COMPONENTNAME>>Inventory} /
 * {@code <<COMPONENTNAME>>Poll} / {@code <<COMPONENTNAME>>Publish} families next to the two in
 * {@link #familyDefs()} — see {@code modbus-adapter/modbus_adapter/metrics.py} and
 * {@code ethernet-ip-adapter/crates/ethernet-ip-adapter/src/metrics.rs} for the full worked set (poll
 * cycles, samples good/bad/uncertain/changed/suppressed, batch flushes, …). Register each new family
 * in {@link #familyDefs()} and pre-define it in {@link DeviceMetrics#defineAll()}; the rest of the
 * pattern (record → drain → emit) is copy-shaped from the {@code Command} family.
 */
public final class Metrics {

    private Metrics() {
    }

    /** The metric every southbound adapter emits (SOUTHBOUND.md §5). */
    public static final String HEALTH = "southbound_health";
    /**
     * The worked operational family for the connect/reconnect lifecycle. Named from the component so a
     * fleet view can tell one adapter's connection health from another's.
     */
    public static final String CONNECTION = "<<COMPONENTNAME>>Connection";
    /** The worked operational family for the {@code sb/*} surface, dimensioned {@code instance}×{@code verb}×{@code result}. */
    public static final String COMMAND = "<<COMPONENTNAME>>Command";

    /** A {@code result} dimension value: the operation succeeded. */
    public static final String RESULT_SUCCESS = "success";
    /** A {@code result} dimension value: the operation failed. */
    public static final String RESULT_ERROR = "error";
    static final String[] RESULTS = {RESULT_SUCCESS, RESULT_ERROR};

    /**
     * The <b>closed</b> {@code verb} dimension set for {@link #COMMAND} — every {@code sb/*} verb the
     * command surface registers ({@code Commands.java}). Closed and low-cardinality on purpose.
     */
    public static final String[] COMMAND_VERBS = {
            "sb/status", "sb/read", "sb/write", "sb/signals", "sb/browse", "sb/pause", "sb/resume",
            "reconnect", "repoll",
    };

    /**
     * The <b>exact</b> SOUTHBOUND.md §5 measure set of {@code southbound_health} — {@code connectionState},
     * {@code publishLatencyMs}, {@code pollLatencyMs}, {@code readErrors}, {@code staleSignals}, plus the
     * §5-optional {@code reconnects}. This literal list is the parity anchor the metrics test asserts
     * against; if you change what {@link DeviceMetrics#emitPeriodic()} emits, this list and
     * {@link #familyDefs()} must move with it.
     */
    public static final String[] HEALTH_MEASURES = {
            "connectionState", "publishLatencyMs", "pollLatencyMs", "readErrors", "staleSignals", "reconnects",
    };

    static final String UNIT_COUNT = "Count";
    static final String UNIT_MS = "Milliseconds";

    /** One measure's name, unit, and storage resolution. */
    record MeasureDef(String name, String unit, int res) {
    }

    /** One metric family's full definition: its name, dimension keys, and measures. */
    record FamilyDef(String name, List<String> dimensions, List<MeasureDef> measures) {
    }

    private static MeasureDef m(String name, String unit, int res) {
        return new MeasureDef(name, unit, res);
    }

    /** A {@code <prefix>Total} + {@code <prefix>Interval} counter pair (both {@code Count}, resolution 60). */
    private static List<MeasureDef> pairDefs(String prefix) {
        List<MeasureDef> out = new ArrayList<>();
        out.add(m(prefix + "Total", UNIT_COUNT, 60));
        out.add(m(prefix + "Interval", UNIT_COUNT, 60));
        return out;
    }

    /**
     * The <b>complete</b> definition set — every family, measure, and dimension key this adapter emits.
     * The startup pre-definition ({@link DeviceMetrics#defineAll()}) and the parity test both read it,
     * so a dropped or renamed measure fails the build.
     */
    public static List<FamilyDef> familyDefs() {
        List<FamilyDef> out = new ArrayList<>();

        // southbound_health — the §5 canonical set (dims: instance). All single measures.
        out.add(new FamilyDef(HEALTH, List.of("instance"), List.of(
                m("connectionState", UNIT_COUNT, 1),
                m("publishLatencyMs", UNIT_MS, 1),
                m("pollLatencyMs", UNIT_MS, 1),
                m("readErrors", UNIT_COUNT, 60),
                m("staleSignals", UNIT_COUNT, 60),
                m("reconnects", UNIT_COUNT, 60))));

        // <<COMPONENTNAME>>Connection — the connect/reconnect lifecycle (dims: instance).
        List<MeasureDef> conn = new ArrayList<>();
        conn.add(m("connectionState", UNIT_COUNT, 1));
        conn.addAll(pairDefs("connectAttempts"));
        conn.addAll(pairDefs("connectFailures"));
        conn.addAll(pairDefs("reconnectAttempts"));
        conn.addAll(pairDefs("connectionDrops"));
        conn.add(m("connectedDurationMs", UNIT_MS, 60));
        out.add(new FamilyDef(CONNECTION, List.of("instance"), conn));

        // <<COMPONENTNAME>>Command — the sb/* surface (dims: instance, verb, result).
        List<MeasureDef> cmd = new ArrayList<>();
        cmd.addAll(pairDefs("commandRequests"));
        cmd.addAll(pairDefs("commandErrors"));
        cmd.add(m("commandLatencyMs", UNIT_MS, 60));
        out.add(new FamilyDef(COMMAND, List.of("instance", "verb", "result"), cmd));

        // ADD YOUR PROTOCOL'S FAMILIES HERE (Inventory / Poll / Publish — see the class header).

        return out;
    }

    static FamilyDef familyDef(String name) {
        for (FamilyDef f : familyDefs()) {
            if (f.name().equals(name)) {
                return f;
            }
        }
        throw new IllegalStateException("familyDefs() covers every family the emitter uses: " + name);
    }

    /**
     * How many signals are stale: those whose last update is older than {@code staleAfterNanos}. The
     * pure core of {@code southbound_health.staleSignals} — a signal that silently stops updating is
     * otherwise indistinguishable from one that is simply not changing, which is why it is measured.
     */
    static long countStale(Iterable<Long> lastUpdateNanos, long nowNanos, long staleAfterNanos) {
        long n = 0;
        for (long t : lastUpdateNanos) {
            if (nowNanos - t > staleAfterNanos) {
                n++;
            }
        }
        return n;
    }

    /** A {@code <name>Total} (monotonic) + {@code <name>Interval} (reset on emit) counter pair. */
    static final class Pair {
        double total;
        double interval;

        void add(double v) {
            total += v;
            interval += v;
        }

        /** Write both measures into {@code out} and <b>reset the interval</b> — the emit convention. */
        void drainInto(Map<String, Float> out, String prefix) {
            out.put(prefix + "Total", (float) total);
            out.put(prefix + "Interval", (float) interval);
            interval = 0.0;
        }
    }
}

/**
 * A per-device operational-metrics emitter. Owns the counter state for one device's
 * {@code southbound_health} plus the two worked families, and emits them on the metrics cadence and on
 * connect/disconnect transitions. One per configured instance.
 */
final class DeviceMetrics {

    private static final Logger LOGGER = LogManager.getLogger(DeviceMetrics.class);

    private final MetricEmitter svc;
    private final ConfigManager config;
    private final String instance;
    private final Health health;
    /**
     * A signal with no update for longer than this (in nanos) is counted in {@code staleSignals}
     * ({@code component.global.healthThresholds.staleSignalSecs}).
     */
    private final long staleAfterNanos;

    private final Object sync = new Object();
    private final ConnCounters conn = new ConnCounters();
    /** verb -> result -> counters; the full matrix is pre-populated so the dimension set is fixed. */
    private final Map<String, Map<String, CmdCounters>> command = new LinkedHashMap<>();
    /** Per-signal last-update instant (nanos) — the staleness tracker driving {@code staleSignals}. */
    private final Map<String, Long> lastUpdateNanos = new HashMap<>();

    DeviceMetrics(MetricEmitter svc, ConfigManager config, String instance, Health health,
                  long staleSignalSecs) {
        this.svc = svc;
        this.config = config;
        this.instance = instance;
        this.health = health;
        this.staleAfterNanos = Math.max(1L, staleSignalSecs) * 1_000_000_000L;
        for (String verb : Metrics.COMMAND_VERBS) {
            Map<String, CmdCounters> byResult = new LinkedHashMap<>();
            for (String result : Metrics.RESULTS) {
                byResult.put(result, new CmdCounters());
            }
            command.put(verb, byResult);
        }
    }

    // ---- recording (called from the device task / command threads) -------------------------------

    /** A connect attempt is about to be made. */
    void onConnectAttempt() {
        synchronized (sync) {
            conn.connectAttempts.add(1.0);
        }
    }

    /**
     * The connect attempt succeeded. A re-establishment (after a previous drop) also bumps
     * {@code reconnectAttempts}.
     */
    void onConnected(long nowNanos) {
        synchronized (sync) {
            conn.connectedSinceNanos = nowNanos;
            conn.connectedSincePresent = true;
            if (conn.everConnected) {
                conn.reconnectAttempts.add(1.0);
            }
            conn.everConnected = true;
        }
    }

    /** The connect attempt failed (unreachable / refused / timeout). */
    void onConnectFailure() {
        synchronized (sync) {
            conn.connectFailures.add(1.0);
        }
    }

    /** An established session was lost. */
    void onConnectionDropped(long nowNanos) {
        synchronized (sync) {
            conn.accrue(nowNanos);
            conn.connectedSincePresent = false;
            conn.connectionDrops.add(1.0);
        }
    }

    /** Note that a signal just updated — feeds the {@code staleSignals} tracker. */
    void onSignalUpdate(String signalId, long nowNanos) {
        synchronized (sync) {
            lastUpdateNanos.put(signalId, nowNanos);
        }
    }

    /** Record one {@code sb/*} command outcome for its {@code (verb, result)} combo. */
    void recordCommand(String verb, boolean ok, long latencyMs) {
        String result = ok ? Metrics.RESULT_SUCCESS : Metrics.RESULT_ERROR;
        synchronized (sync) {
            CmdCounters c = command.get(verb).get(result);
            c.commandRequests.add(1.0);
            c.commandLatencyMs += latencyMs;
            if (!ok) {
                c.commandErrors.add(1.0);
            }
        }
    }

    /**
     * The connection-counter snapshot for {@code sb/status} / the diagnostics panel: each counter as
     * {@code {interval, total}}. Cheap; no device I/O.
     */
    JsonObject countersView() {
        synchronized (sync) {
            JsonObject out = new JsonObject();
            out.add("connectAttempts", pairView(conn.connectAttempts));
            out.add("connectFailures", pairView(conn.connectFailures));
            out.add("reconnectAttempts", pairView(conn.reconnectAttempts));
            out.add("connectionDrops", pairView(conn.connectionDrops));
            return out;
        }
    }

    private static JsonObject pairView(Metrics.Pair p) {
        JsonObject o = new JsonObject();
        o.addProperty("interval", p.interval);
        o.addProperty("total", p.total);
        return o;
    }

    private double staleCount(long nowNanos) {
        synchronized (sync) {
            return (double) Metrics.countStale(lastUpdateNanos.values(), nowNanos, staleAfterNanos);
        }
    }

    // ---- definition + emission -------------------------------------------------------------------

    /**
     * Pre-define every family × dimension combination at startup, so the metric set is fixed and
     * discoverable. Each is also re-defined immediately before each emit (the name-keyed-store rule).
     */
    void defineAll() {
        define(Metrics.HEALTH, dims("instance", instance));
        define(Metrics.CONNECTION, dims("instance", instance));
        for (String verb : Metrics.COMMAND_VERBS) {
            for (String result : Metrics.RESULTS) {
                define(Metrics.COMMAND, dims("instance", instance, "verb", verb, "result", result));
            }
        }
    }

    private static Map<String, String> dims(String... kv) {
        Map<String, String> out = new LinkedHashMap<>();
        for (int i = 0; i + 1 < kv.length; i += 2) {
            out.put(kv[i], kv[i + 1]);
        }
        return out;
    }

    /** Build + register one family combo's metric definition. */
    private void define(String name, Map<String, String> dimensions) {
        Metrics.FamilyDef def = Metrics.familyDef(name);
        MetricBuilder b = MetricBuilder.create(name).withConfig(config);
        for (Metrics.MeasureDef measure : def.measures()) {
            b = b.addMeasure(measure.name(), measure.unit(), measure.res());
        }
        for (Map.Entry<String, String> d : dimensions.entrySet()) {
            b = b.addDimension(d.getKey(), d.getValue());
        }
        svc.defineMetric(b.build());
    }

    /** Re-define (with the combo's dimensions) then emit one family combo. */
    private void emitCombo(String name, Map<String, String> dimensions, Map<String, Float> values,
                           boolean now) {
        define(name, dimensions);
        try {
            if (now) {
                svc.emitMetricNow(name, values);
            } else {
                svc.emitMetric(name, values);
            }
        } catch (RuntimeException e) {
            LOGGER.warn("metric emit failed for {} (instance {}): {}", name, instance, e.toString());
        }
    }

    /**
     * The full periodic emit (every metrics interval): {@code southbound_health}, the connection
     * family, and every command {@code (verb, result)} combo.
     */
    void emitPeriodic() {
        emitHealth(false);
        emitConnection(false);
        emitCommand();
    }

    /**
     * The immediate transition emit ({@code emitMetricNow}): the mandatory {@code southbound_health}
     * plus the connection gauges whose state just changed — flushed on connect / disconnect.
     */
    void emitNow() {
        emitHealth(true);
        emitConnection(true);
    }

    private void emitHealth(boolean now) {
        Map<String, Float> v = new LinkedHashMap<>();
        v.put("connectionState", (float) health.connectionState());
        v.put("publishLatencyMs", (float) health.publishLatencyMs());
        v.put("pollLatencyMs", (float) health.pollLatencyMs());
        v.put("readErrors", (float) health.takeReadErrors());
        v.put("staleSignals", (float) staleCount(System.nanoTime()));
        v.put("reconnects", (float) health.takeReconnects());
        emitCombo(Metrics.HEALTH, dims("instance", instance), v, now);
    }

    private void emitConnection(boolean now) {
        Map<String, Float> values;
        double state;
        synchronized (sync) {
            state = health.connectionState();
            values = conn.drain(System.nanoTime(), state);
        }
        emitCombo(Metrics.CONNECTION, dims("instance", instance), values, now);
    }

    private void emitCommand() {
        Map<String, Map<String, Map<String, Float>>> rows = new LinkedHashMap<>();
        synchronized (sync) {
            for (Map.Entry<String, Map<String, CmdCounters>> byVerb : command.entrySet()) {
                Map<String, Map<String, Float>> byResult = new LinkedHashMap<>();
                for (Map.Entry<String, CmdCounters> e : byVerb.getValue().entrySet()) {
                    byResult.put(e.getKey(), e.getValue().drain());
                }
                rows.put(byVerb.getKey(), byResult);
            }
        }
        for (Map.Entry<String, Map<String, Map<String, Float>>> byVerb : rows.entrySet()) {
            for (Map.Entry<String, Map<String, Float>> e : byVerb.getValue().entrySet()) {
                emitCombo(Metrics.COMMAND,
                        dims("instance", instance, "verb", byVerb.getKey(), "result", e.getKey()),
                        e.getValue(), false);
            }
        }
    }

    /** The connect/reconnect lifecycle counters. */
    private static final class ConnCounters {
        boolean everConnected;
        final Metrics.Pair connectAttempts = new Metrics.Pair();
        final Metrics.Pair connectFailures = new Metrics.Pair();
        final Metrics.Pair reconnectAttempts = new Metrics.Pair();
        final Metrics.Pair connectionDrops = new Metrics.Pair();
        double connectedAccruedMs;
        boolean connectedSincePresent;
        long connectedSinceNanos;

        void accrue(long nowNanos) {
            if (connectedSincePresent) {
                connectedAccruedMs += Math.max(0L, nowNanos - connectedSinceNanos) / 1_000_000.0;
                connectedSinceNanos = nowNanos;
            }
        }

        Map<String, Float> drain(long nowNanos, double connectionState) {
            accrue(nowNanos);
            Map<String, Float> v = new LinkedHashMap<>();
            v.put("connectionState", (float) connectionState);
            connectAttempts.drainInto(v, "connectAttempts");
            connectFailures.drainInto(v, "connectFailures");
            reconnectAttempts.drainInto(v, "reconnectAttempts");
            connectionDrops.drainInto(v, "connectionDrops");
            v.put("connectedDurationMs", (float) connectedAccruedMs);
            connectedAccruedMs = 0.0;
            return v;
        }
    }

    /** The {@code sb/*} command counters for one {@code (verb, result)} combo. */
    private static final class CmdCounters {
        final Metrics.Pair commandRequests = new Metrics.Pair();
        final Metrics.Pair commandErrors = new Metrics.Pair();
        double commandLatencyMs;

        Map<String, Float> drain() {
            Map<String, Float> v = new LinkedHashMap<>();
            commandRequests.drainInto(v, "commandRequests");
            commandErrors.drainInto(v, "commandErrors");
            v.put("commandLatencyMs", (float) commandLatencyMs);
            commandLatencyMs = 0.0;
            return v;
        }
    }
}
