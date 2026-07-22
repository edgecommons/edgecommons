package <<PACKAGE>>;

import org.junit.jupiter.api.Test;

import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.TreeSet;
import java.util.stream.Collectors;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Operational metrics parity: {@code southbound_health} is <b>exactly</b> the SOUTHBOUND.md §5 set, and
 * the two worked operational families follow the Total/Interval counter-pair pattern with
 * low-cardinality dimensions only.
 */
class MetricsTest {

    @Test
    void southboundHealthEmitsExactlyTheSection5MeasureSet() {
        // A second, independent copy of §5 — NOT the module const, so a wrong edit to one is caught.
        Set<String> section5 = new TreeSet<>(List.of(
                "connectionState", "publishLatencyMs", "pollLatencyMs", "readErrors", "staleSignals",
                "reconnects"));

        Metrics.FamilyDef health = Metrics.familyDefs().stream()
                .filter(f -> f.name().equals(Metrics.HEALTH)).findFirst().orElseThrow();
        Set<String> emitted = health.measures().stream().map(Metrics.MeasureDef::name)
                .collect(Collectors.toCollection(TreeSet::new));
        assertEquals(section5, emitted,
                "southbound_health must be the exact §5 set — no more, no less");

        Set<String> advertised = new TreeSet<>(List.of(Metrics.HEALTH_MEASURES));
        assertEquals(section5, advertised, "HEALTH_MEASURES must equal the §5 set");
    }

    @Test
    void operationalFamiliesAreNamedFromTheComponentAndLowCardinality() {
        List<Metrics.FamilyDef> defs = Metrics.familyDefs();
        List<String> names = defs.stream().map(Metrics.FamilyDef::name).toList();
        assertTrue(names.contains(Metrics.CONNECTION), "the Connection family is present");
        assertTrue(names.contains(Metrics.COMMAND), "the Command family is present");
        // Named from the component token — a fleet view separates adapters by name.
        assertTrue(Metrics.CONNECTION.endsWith("Connection") && !Metrics.CONNECTION.equals("Connection"));
        assertTrue(Metrics.COMMAND.endsWith("Command") && !Metrics.COMMAND.equals("Command"));

        Metrics.FamilyDef cmd = defs.stream().filter(f -> f.name().equals(Metrics.COMMAND))
                .findFirst().orElseThrow();
        assertEquals(List.of("instance", "verb", "result"), cmd.dimensions(),
                "closed, low-cardinality dims only");
    }

    @Test
    void theConnectionFamilyIsTheCounterPairPattern() {
        Metrics.FamilyDef conn = Metrics.familyDefs().stream()
                .filter(f -> f.name().equals(Metrics.CONNECTION)).findFirst().orElseThrow();
        List<String> names = conn.measures().stream().map(Metrics.MeasureDef::name).toList();
        for (String base : List.of("connectAttempts", "connectFailures", "reconnectAttempts",
                "connectionDrops")) {
            assertTrue(names.contains(base + "Total"), base + "Total present");
            assertTrue(names.contains(base + "Interval"), base + "Interval present");
        }
        assertTrue(names.contains("connectionState"), "the state gauge");
        assertTrue(names.contains("connectedDurationMs"), "the connected-duration sum");
    }

    @Test
    void intervalCountersResetOnDrainButTotalsDoNot() {
        Metrics.Pair p = new Metrics.Pair();
        p.add(3.0);
        Map<String, Float> out = new HashMap<>();
        p.drainInto(out, "x");
        assertEquals(3.0f, out.get("xTotal"));
        assertEquals(3.0f, out.get("xInterval"));

        p.add(2.0);
        Map<String, Float> out2 = new HashMap<>();
        p.drainInto(out2, "x");
        assertEquals(5.0f, out2.get("xTotal"), "total is monotonic across emits");
        assertEquals(2.0f, out2.get("xInterval"),
                "interval resets to only what accrued since the last emit");
    }

    @Test
    void staleSignalsCountsOnlySignalsPastTheThreshold() {
        long now = System.nanoTime();
        long thirtySecs = 30L * 1_000_000_000L;
        List<Long> lastUpdate = List.of(now, now - 120L * 1_000_000_000L);
        assertEquals(1L, Metrics.countStale(lastUpdate, now, thirtySecs),
                "only the signal older than staleSignalSecs is stale");
    }

    @Test
    void everyCommandVerbHasAMeasureBucketAndThereAreExactlyNine() {
        assertEquals(9, Metrics.COMMAND_VERBS.length);
        Set<String> verbs = new TreeSet<>(List.of(Metrics.COMMAND_VERBS));
        assertEquals(new TreeSet<>(List.of("sb/status", "sb/read", "sb/write", "sb/signals",
                "sb/browse", "sb/pause", "sb/resume", "reconnect", "repoll")), verbs);
    }
}
