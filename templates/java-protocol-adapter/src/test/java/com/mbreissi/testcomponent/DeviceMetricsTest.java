package <<PACKAGE>>;

import com.mbreissi.edgecommons.config.ConfigManager;
import com.mbreissi.edgecommons.config.ConfigurationFactory;
import com.mbreissi.edgecommons.config.MetricConfiguration;
import com.mbreissi.edgecommons.metrics.MetricEmitter;
import com.google.gson.JsonObject;
import org.junit.jupiter.api.Test;

import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * The per-device operational-metrics emitter: pre-definition of every family, the recording of the
 * connect/command counters, and the Total/Interval drain convention on emit — exercised end to end
 * against a recording {@link MetricEmitter} and a minimal config, with no broker. The counter math is
 * the point; the live push to a metric target is stubbed, but every define / record / drain / emit
 * path in {@link DeviceMetrics} runs here.
 */
class DeviceMetricsTest {

    /**
     * A {@link MetricEmitter} that records what was emitted (so the drain math can be asserted) and
     * defines into the inherited map (no live target — {@code define} needs none). Built on the
     * library's {@code protected} no-arg constructor, the sanctioned test seam.
     */
    private static final class RecordingEmitter extends MetricEmitter {
        final List<String> emitted = new ArrayList<>();
        final Map<String, Map<String, Float>> last = new LinkedHashMap<>();

        @Override
        public void emitMetric(String name, Map<String, Float> values) {
            emitted.add(name);
            last.put(name, new LinkedHashMap<>(values));
        }

        @Override
        public void emitMetricNow(String name, Map<String, Float> values) {
            emitMetric(name, values);
        }
    }

    /** A config just complete enough for {@code MetricBuilder.withConfig(...)}: names + a namespace. */
    private static ConfigManager config() {
        MetricConfiguration mc = ConfigurationFactory.createMetricConfiguration(new JsonObject());
        return new ConfigManager() {
            @Override
            public String getThingName() {
                return "test-thing";
            }

            @Override
            public String getComponentName() {
                return "test-component";
            }

            @Override
            public MetricConfiguration getMetricConfig() {
                return mc;
            }
        };
    }

    private static DeviceMetrics deviceMetrics(RecordingEmitter emitter, Health health, long staleSecs) {
        DeviceMetrics dm = new DeviceMetrics(emitter, config(), "plc-1", health, staleSecs);
        dm.defineAll();
        return dm;
    }

    @Test
    void defineAllPreRegistersEveryFamily() {
        RecordingEmitter emitter = new RecordingEmitter();
        deviceMetrics(emitter, new Health(), 30);
        // isMetricDefined reads the inherited definition map populated by defineAll -> define.
        assertTrue(emitter.isMetricDefined(Metrics.HEALTH), "southbound_health is pre-defined at startup");
        assertTrue(emitter.isMetricDefined(Metrics.CONNECTION));
        assertTrue(emitter.isMetricDefined(Metrics.COMMAND));
    }

    @Test
    void thePeriodicEmitCarriesHealthConnectionAndEveryCommandCombo() {
        RecordingEmitter emitter = new RecordingEmitter();
        Health health = new Health();
        health.setLink(LinkState.ONLINE);
        health.setPollLatencyMs(12);
        health.setPublishLatencyMs(7);
        health.incrementReadErrors();
        health.incrementReconnects();
        DeviceMetrics dm = deviceMetrics(emitter, health, 30);

        dm.onConnectAttempt();
        dm.onConnected(System.nanoTime());
        dm.onSignalUpdate("temperature-1", System.nanoTime());
        dm.recordCommand("sb/read", true, 4);
        dm.recordCommand("sb/write", false, 9);

        emitter.emitted.clear();
        dm.emitPeriodic();

        assertTrue(emitter.emitted.contains(Metrics.HEALTH), "the §5 health metric is always emitted");
        assertTrue(emitter.emitted.contains(Metrics.CONNECTION));
        long commandEmits = emitter.emitted.stream().filter(Metrics.COMMAND::equals).count();
        assertEquals(Metrics.COMMAND_VERBS.length * 2, commandEmits,
                "one command emit per (verb, result) combo — the dimension matrix is fixed");

        Map<String, Float> h = emitter.last.get(Metrics.HEALTH);
        assertEquals(1.0f, h.get("connectionState"), "online reads as connectionState=1");
        assertEquals(12.0f, h.get("pollLatencyMs"));
        assertEquals(1.0f, h.get("readErrors"), "the read-error interval counter is drained into the emit");
        assertEquals(1.0f, h.get("reconnects"));
    }

    @Test
    void intervalCountersResetAcrossEmitsButTotalsAccumulate() {
        RecordingEmitter emitter = new RecordingEmitter();
        DeviceMetrics dm = deviceMetrics(emitter, new Health(), 30);

        dm.onConnectAttempt();
        dm.onConnectAttempt();
        dm.emitPeriodic();
        Map<String, Float> first = emitter.last.get(Metrics.CONNECTION);
        assertEquals(2.0f, first.get("connectAttemptsTotal"));
        assertEquals(2.0f, first.get("connectAttemptsInterval"));

        dm.onConnectAttempt();
        dm.emitPeriodic();
        Map<String, Float> second = emitter.last.get(Metrics.CONNECTION);
        assertEquals(3.0f, second.get("connectAttemptsTotal"), "total is monotonic across emits");
        assertEquals(1.0f, second.get("connectAttemptsInterval"),
                "interval reset to only what accrued since the last emit");
    }

    @Test
    void aConnectionDropAccruesConnectedTimeAndCountsTheDropAndFailure() {
        RecordingEmitter emitter = new RecordingEmitter();
        DeviceMetrics dm = deviceMetrics(emitter, new Health(), 30);

        long t0 = System.nanoTime();
        dm.onConnectAttempt();
        dm.onConnected(t0);
        dm.onConnectFailure();
        dm.onConnectionDropped(t0 + 5_000_000L); // 5 ms connected before the drop
        dm.emitNow(); // the transition flush: health + connection, emitted immediately

        // countersView is the sb/status snapshot: each counter as {interval, total}.
        JsonObject view = dm.countersView();
        assertTrue(view.getAsJsonObject("connectionDrops").get("total").getAsDouble() >= 1.0);
        assertTrue(view.getAsJsonObject("connectFailures").get("total").getAsDouble() >= 1.0);
        assertTrue(emitter.emitted.contains(Metrics.HEALTH), "emitNow flushes the mandatory health metric");
    }

    @Test
    void staleSignalsCountsSignalsThatStoppedUpdating() {
        RecordingEmitter emitter = new RecordingEmitter();
        DeviceMetrics dm = deviceMetrics(emitter, new Health(), 1); // 1-second staleness threshold

        // A signal last seen two seconds ago is stale; the counter is how a silent stop becomes visible.
        dm.onSignalUpdate("temperature-1", System.nanoTime() - 2_000_000_000L);
        dm.emitPeriodic();
        assertEquals(1.0f, emitter.last.get(Metrics.HEALTH).get("staleSignals"));
    }
}
