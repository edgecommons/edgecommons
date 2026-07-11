package <<PACKAGE>>;

import com.google.gson.JsonObject;

import java.util.Set;

/**
 * One sink instance == one entry of {@code component.instances[]}.
 *
 * <p>Parsing is <b>strict</b>: an unknown key is rejected rather than ignored, because a typo'd key
 * in a sink's config is how data quietly goes to the wrong place.
 *
 * @param id          unique sink id — the {@code {instance}} token of its UNS topics, and the prefix
 *                    of every destination key it writes, so it must be stable
 * @param subscribe   the topic filter whose messages this sink delivers
 * @param destination where they go (a tagged object; see {@link Destination#build(JsonObject)})
 * @param retry       how hard, and for how long, to keep trying
 * @param maxQueue    bounded, like every queue that faces a network
 */
public record SinkConfig(
        String id,
        String subscribe,
        JsonObject destination,
        RetryConfig retry,
        int maxQueue) {

    /** Bounded, like every queue that faces a network. */
    public static final int DEFAULT_MAX_QUEUE = 256;

    private static final Set<String> SINK_KEYS = Set.of("id", "subscribe", "destination", "retry", "maxQueue");

    /**
     * How hard, and for how long, to keep trying.
     *
     * <p>Note the give-up is a <b>time budget</b>, not an attempt count. "Twenty attempts" means
     * something different at 1 s and at 15 min of backoff; "keep trying for an hour" means the same
     * thing at every cadence, and it is what an operator can actually reason about.
     *
     * @param baseDelayMs   the first backoff window; each attempt doubles it, up to {@code maxDelayMs}
     * @param maxDelayMs    the backoff ceiling, so a long outage does not back off to next week
     * @param giveUpAfterMs the TIME BUDGET — when it is spent, the item is reported as exhausted
     */
    public record RetryConfig(long baseDelayMs, long maxDelayMs, long giveUpAfterMs) {

        public static final long DEFAULT_BASE_DELAY_MS = 1_000;
        public static final long DEFAULT_MAX_DELAY_MS = 900_000;      // 15 min
        public static final long DEFAULT_GIVE_UP_AFTER_MS = 3_600_000; // 1 hour

        private static final Set<String> RETRY_KEYS = Set.of("baseDelayMs", "maxDelayMs", "giveUpAfterMs");

        /** The built-in policy, used when neither the sink nor `component.global.defaults` says. */
        public static RetryConfig defaults() {
            return new RetryConfig(DEFAULT_BASE_DELAY_MS, DEFAULT_MAX_DELAY_MS, DEFAULT_GIVE_UP_AFTER_MS);
        }

        /**
         * Full-jitter exponential backoff: a delay uniformly drawn from
         * {@code [0, min(cap, base * 2^attempt))}.
         *
         * <p>The jitter is not decoration. Without it, every component that lost the same endpoint
         * retries at the same instant, and the endpoint — which is probably struggling already — is
         * hit by a synchronized thundering herd on every backoff boundary.
         *
         * @param attempt the zero-based attempt number
         * @param rand01  a uniform sample in {@code [0, 1]} — a parameter, so the jitter is testable
         * @return the delay in milliseconds
         */
        public long delayMs(int attempt, double rand01) {
            int shift = Math.min(Math.max(attempt, 0), 20); // saturate: 2^20 * base already exceeds any cap
            long exp = baseDelayMs << shift;
            long cap = Math.min(exp < 0 ? Long.MAX_VALUE : exp, maxDelayMs);
            return (long) (Math.clamp(rand01, 0.0, 1.0) * cap);
        }

        /** Has the time budget run out? This — not an attempt count — is the give-up. */
        public boolean budgetSpent(long elapsedMs) {
            return elapsedMs >= giveUpAfterMs;
        }

        /** Parses a `retry` object, layering it over {@code fallback} key by key. */
        public static RetryConfig parse(JsonObject o, RetryConfig fallback) {
            if (o == null) {
                return fallback;
            }
            for (String key : o.keySet()) {
                if (!RETRY_KEYS.contains(key)) {
                    throw new IllegalArgumentException("unknown retry key `" + key + "`; expected one of " + RETRY_KEYS);
                }
            }
            return new RetryConfig(
                    o.has("baseDelayMs") ? o.get("baseDelayMs").getAsLong() : fallback.baseDelayMs(),
                    o.has("maxDelayMs") ? o.get("maxDelayMs").getAsLong() : fallback.maxDelayMs(),
                    o.has("giveUpAfterMs") ? o.get("giveUpAfterMs").getAsLong() : fallback.giveUpAfterMs());
        }
    }

    /**
     * Parses one entry of {@code component.instances[]}.
     *
     * @param instance       the instance object, as the library handed it over
     * @param globalDefaults the object at {@code component.global.defaults}, or {@code null}
     * @return the parsed sink
     * @throws IllegalArgumentException on a missing required key or an unknown key
     */
    public static SinkConfig parse(JsonObject instance, JsonObject globalDefaults) {
        for (String key : instance.keySet()) {
            if (!SINK_KEYS.contains(key)) {
                throw new IllegalArgumentException("unknown sink key `" + key + "`; expected one of " + SINK_KEYS);
            }
        }
        String id = requireString(instance, "id");
        String subscribe = requireString(instance, "subscribe");
        if (!instance.has("destination") || !instance.get("destination").isJsonObject()) {
            throw new IllegalArgumentException("sink `" + id + "` is missing its `destination` object");
        }
        JsonObject destination = instance.getAsJsonObject("destination");

        RetryConfig baseline = RetryConfig.defaults();
        int maxQueue = DEFAULT_MAX_QUEUE;
        if (globalDefaults != null) {
            baseline = RetryConfig.parse(
                    globalDefaults.has("retry") ? globalDefaults.getAsJsonObject("retry") : null, baseline);
            if (globalDefaults.has("maxQueue")) {
                maxQueue = globalDefaults.get("maxQueue").getAsInt();
            }
        }
        RetryConfig retry = RetryConfig.parse(
                instance.has("retry") ? instance.getAsJsonObject("retry") : null, baseline);
        if (instance.has("maxQueue")) {
            maxQueue = instance.get("maxQueue").getAsInt();
        }

        return new SinkConfig(id, subscribe, destination, retry, maxQueue);
    }

    private static String requireString(JsonObject o, String key) {
        if (!o.has(key) || o.get(key).isJsonNull()) {
            throw new IllegalArgumentException("sink is missing the required key `" + key + "`");
        }
        return o.get(key).getAsString();
    }
}
