package <<PACKAGE>>;

import com.google.gson.JsonObject;
import com.mbreissi.edgecommons.facades.Severity;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import java.nio.charset.StandardCharsets;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * The deliver → verify → retry ladder — the sink's archetype, exercised with a recording reporter
 * and a scripted destination, no broker. The event ladder is the sink's contract with whoever is
 * watching, so it is asserted here as precisely as the return value: an operator must be able to tell
 * "still trying" from "gave up", and gave-up must be Critical, because it is data that did not arrive.
 */
class DeliveryTest {

    // --- a reporter that records every event + connectivity transition -----------------------------

    private record Event(Severity severity, String type, String message) {
    }

    private record Conn(boolean reachable, String state, String detail) {
    }

    private static final class RecordingReporter implements Delivery.Reporter {
        final List<Event> events = new ArrayList<>();
        final List<Conn> conns = new ArrayList<>();

        @Override
        public void event(Severity severity, String type, String message, JsonObject context) {
            events.add(new Event(severity, type, message));
        }

        @Override
        public void connectivity(boolean reachable, String state, String detail) {
            conns.add(new Conn(reachable, state, detail));
        }

        List<String> types() {
            return events.stream().map(Event::type).toList();
        }
    }

    // --- a destination scripted to fail a configurable number of times before it succeeds ----------

    private static final class ScriptedDestination implements Destination {
        int transientFailuresBeforeSuccess = 0;
        boolean permanent = false;
        int delivered = 0;

        @Override
        public String kind() {
            return "scripted";
        }

        @Override
        public Delivered deliver(Item item) throws DeliverException {
            if (permanent) {
                throw DeliverException.permanentFailure("bad credentials");
            }
            if (transientFailuresBeforeSuccess > 0) {
                transientFailuresBeforeSuccess--;
                throw DeliverException.transientFailure("connection reset");
            }
            delivered++;
            return new Delivered(item.bytes().length);
        }

        @Override
        public void verify(Item item, Delivered d) {
            // A scripted destination that returned from deliver() is trusted to have landed the bytes.
        }
    }

    private static SinkConfig sink(long giveUpAfterMs) {
        return new SinkConfig("archive", "ecv1/+/+/+/data/#", new JsonObject(),
                new SinkConfig.RetryConfig(1, 2, giveUpAfterMs), 16);
    }

    private static Item item() {
        return new Item("archive/x/1.json", "{\"v\":1}".getBytes(StandardCharsets.UTF_8));
    }

    @Test
    void aVerifiedDeliveryReportsStartedThenCompletedAndReleasesTheSource(@TempDir Path dir) {
        Stats stats = new Stats();
        RecordingReporter r = new RecordingReporter();

        boolean ok = Delivery.deliverWithRetry(sink(3_600_000), item(), new LocalDestination(dir), stats, r);

        assertTrue(ok, "a verified delivery is the only true delivery");
        assertEquals(1, stats.delivered.get());
        assertEquals(0, stats.retried.get());
        assertEquals(0, stats.exhausted.get());
        assertEquals(List.of("delivery-started", "delivery-completed"), r.types());
        assertEquals(new Conn(true, "ONLINE", null), r.conns.get(r.conns.size() - 1),
                "a verified delivery is the only proof the destination is reachable");
    }

    @Test
    void aPermanentFailureGivesUpAtOnceAndIsReportedCritical() {
        Stats stats = new Stats();
        RecordingReporter r = new RecordingReporter();
        ScriptedDestination dest = new ScriptedDestination();
        dest.permanent = true;

        boolean ok = Delivery.deliverWithRetry(sink(3_600_000), item(), dest, stats, r);

        assertFalse(ok);
        assertEquals(1, stats.exhausted.get(), "a permanent error is exhausted, never retried");
        assertEquals(0, stats.retried.get(), "retrying a permanent error would waste the budget");
        assertEquals(List.of("delivery-started", "delivery-exhausted"), r.types());
        assertEquals(Severity.CRITICAL, r.events.get(1).severity(), "gave-up must be loud");
        assertEquals(new Conn(false, "FAILED", "bad credentials"), r.conns.get(0));
    }

    @Test
    void transientFailuresAreRetriedUntilOneSucceeds() {
        Stats stats = new Stats();
        RecordingReporter r = new RecordingReporter();
        ScriptedDestination dest = new ScriptedDestination();
        dest.transientFailuresBeforeSuccess = 2; // fail, fail, then land it

        boolean ok = Delivery.deliverWithRetry(sink(3_600_000), item(), dest, stats, r);

        assertTrue(ok, "a transient failure is worth another attempt");
        assertEquals(1, stats.delivered.get());
        assertEquals(2, stats.retried.get(), "two retries before the third attempt landed");
        assertEquals(List.of("delivery-started", "delivery-failed", "delivery-failed", "delivery-completed"),
                r.types());
        // Retrying reports BACKOFF (still trying, data in hand); only the final landing reports ONLINE.
        assertEquals("BACKOFF", r.conns.get(0).state());
        assertEquals("ONLINE", r.conns.get(r.conns.size() - 1).state());
    }

    @Test
    void aSpentTimeBudgetStopsRetryingAndReportsExhausted() {
        Stats stats = new Stats();
        RecordingReporter r = new RecordingReporter();
        ScriptedDestination dest = new ScriptedDestination();
        dest.transientFailuresBeforeSuccess = 1_000; // it would never succeed on its own

        // A zero-length budget: the first transient failure has already spent it, so we give up
        // rather than retry — FAILED, not BACKOFF.
        boolean ok = Delivery.deliverWithRetry(sink(0), item(), dest, stats, r);

        assertFalse(ok);
        assertEquals(1, stats.exhausted.get());
        assertEquals(0, stats.retried.get(), "the budget was already spent, so nothing was retried");
        assertEquals(List.of("delivery-started", "delivery-exhausted"), r.types());
        assertEquals("FAILED", r.conns.get(0).state());
    }
}
