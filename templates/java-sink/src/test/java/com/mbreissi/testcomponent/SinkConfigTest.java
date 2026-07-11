package <<PACKAGE>>;

import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import org.junit.jupiter.api.Test;

import <<PACKAGE>>.SinkConfig.RetryConfig;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

/** The retry policy — the part of a sink that decides whether data survives an outage. */
class SinkConfigTest {

    private static JsonObject json(String s) {
        return JsonParser.parseString(s).getAsJsonObject();
    }

    @Test
    void aSinkParsesFromItsInstanceConfig() {
        SinkConfig sink = SinkConfig.parse(json("""
                {
                  "id": "archive",
                  "subscribe": "ecv1/+/+/+/data/#",
                  "destination": { "type": "local", "path": "/var/lib/out" },
                  "retry": { "baseDelayMs": 500, "giveUpAfterMs": 60000 }
                }
                """), null);

        assertEquals("archive", sink.id());
        assertEquals(500, sink.retry().baseDelayMs());
        assertEquals(RetryConfig.DEFAULT_MAX_DELAY_MS, sink.retry().maxDelayMs(),
                "the unspecified field takes its default");
        assertEquals(SinkConfig.DEFAULT_MAX_QUEUE, sink.maxQueue(), "the queue is bounded by default");
    }

    @Test
    void componentGlobalDefaultsFillWhatTheSinkOmits() {
        JsonObject defaults = json("{\"retry\":{\"baseDelayMs\":250},\"maxQueue\":32}");
        SinkConfig sink = SinkConfig.parse(json("""
                {"id":"a","subscribe":"t","destination":{"type":"local","path":"/o"}}
                """), defaults);
        assertEquals(250, sink.retry().baseDelayMs());
        assertEquals(32, sink.maxQueue());
    }

    @Test
    void backoffGrowsExponentiallyAndIsCapped() {
        RetryConfig r = new RetryConfig(1_000, 10_000, 0);
        // With full jitter, rand01 = 1.0 yields the ceiling of the window.
        assertEquals(1_000, r.delayMs(0, 1.0));
        assertEquals(2_000, r.delayMs(1, 1.0));
        assertEquals(4_000, r.delayMs(2, 1.0));
        // ...and it is capped, so a long outage does not back off to next week.
        assertEquals(10_000, r.delayMs(20, 1.0));
        assertEquals(10_000, r.delayMs(64, 1.0), "the shift saturates rather than wrapping");
    }

    @Test
    void jitterSpreadsTheRetries() {
        // The point of full jitter: two components that lost the same endpoint do NOT retry in
        // lockstep. The delay is a random point INSIDE the window, not the window's edge.
        RetryConfig r = new RetryConfig(1_000, 60_000, 0);
        assertEquals(0, r.delayMs(3, 0.0), "the window's floor is immediate");
        assertEquals(4_000, r.delayMs(3, 0.5), "half way into an 8s window");
        assertEquals(8_000, r.delayMs(3, 1.0));
    }

    @Test
    void theGiveUpIsATimeBudgetNotAnAttemptCount() {
        // "Twenty attempts" means something different at 1s and at 15min of backoff; "keep trying
        // for an hour" means the same thing at every cadence.
        RetryConfig r = new RetryConfig(1, 1, 5_000);
        assertFalse(r.budgetSpent(4_999));
        assertTrue(r.budgetSpent(5_000));
        assertTrue(r.budgetSpent(600_000));
    }

    @Test
    void anUnknownConfigKeyIsRejectedRatherThanIgnored() {
        assertThrows(IllegalArgumentException.class, () -> SinkConfig.parse(json("""
                {"id":"a","subscribe":"t","destination":{"type":"local","path":"/o"},"retires":{}}
                """), null));
        assertThrows(IllegalArgumentException.class, () -> SinkConfig.parse(json("""
                {"id":"a","subscribe":"t","destination":{"type":"local","path":"/o"},
                 "retry":{"giveUpAfterMS":1}}
                """), null));
    }

    @Test
    void aSinkWithoutADestinationIsRejected() {
        assertThrows(IllegalArgumentException.class,
                () -> SinkConfig.parse(json("{\"id\":\"a\",\"subscribe\":\"t\"}"), null));
    }
}
