package <<PACKAGE>>;

import com.google.gson.JsonElement;
import com.google.gson.JsonObject;

import java.util.ArrayList;
import java.util.List;
import java.util.Locale;
import java.util.Set;

/**
 * One route == one entry of {@code component.instances[]}.
 *
 * <p>Routes are independent — one worker thread each — so a slow route cannot stall another, and
 * per-key state inside a stage needs no lock.
 *
 * <p>Parsing is <b>strict</b>: an unknown key is rejected rather than ignored, because a typo'd
 * route key is a mistake and not a no-op. (This is the Java equivalent of serde's
 * {@code deny_unknown_fields}; Gson would silently drop it.)
 *
 * @param id           unique route id — the {@code {instance}} token of this route's UNS topics
 * @param subscribe    topic filters to consume; wildcards are fine ({@code ecv1/+/+/+/data/#})
 * @param publishTopic the topic the transformed result is published on
 * @param target       where the result goes
 * @param pipeline     the stages, in order — an empty pipeline is a pass-through republisher
 * @param maxQueue     how many messages may be queued before new ones are dropped <b>and counted</b>
 * @param tickMs       how often stateful stages are ticked
 */
public record RouteConfig(
        String id,
        List<String> subscribe,
        String publishTopic,
        Target target,
        List<Processor> pipeline,
        int maxQueue,
        long tickMs) {

    /**
     * Bounded on purpose. An unbounded queue does not remove backpressure — it relocates the
     * failure to the heap, and by the time you notice you have lost the ability to report it.
     */
    public static final int DEFAULT_MAX_QUEUE = 256;

    /** How often stateful stages are ticked, when the route does not say. */
    public static final long DEFAULT_TICK_MS = 10_000;

    private static final Set<String> ROUTE_KEYS =
            Set.of("id", "subscribe", "publishTopic", "target", "pipeline", "maxQueue", "tickMs");

    /** Where a route's output goes. */
    public enum Target {
        /** The device-local bus — the common case; another component on this device consumes it. */
        LOCAL,
        /** Straight out to the northbound broker. */
        NORTHBOUND;

        /** The wire token is the lowercase name, matching the config schema's enum. */
        static Target parse(String wire) {
            return valueOf(wire.toUpperCase(Locale.ROOT));
        }
    }

    /**
     * Parses one entry of {@code component.instances[]}.
     *
     * @param instance    the instance object, as the library handed it over
     * @param globalDefaults the object at {@code component.global.defaults}, or {@code null}
     * @return the parsed route
     * @throws IllegalArgumentException on a missing required key, an unknown key, or a stage this
     *                                  component does not implement
     */
    public static RouteConfig parse(JsonObject instance, JsonObject globalDefaults) {
        for (String key : instance.keySet()) {
            if (!ROUTE_KEYS.contains(key)) {
                throw new IllegalArgumentException("unknown route key `" + key + "`; expected one of " + ROUTE_KEYS);
            }
        }
        String id = requireString(instance, "id");
        String publishTopic = requireString(instance, "publishTopic");

        List<String> subscribe = new ArrayList<>();
        if (instance.has("subscribe")) {
            instance.getAsJsonArray("subscribe").forEach(e -> subscribe.add(e.getAsString()));
        }

        Target target = instance.has("target")
                ? Target.parse(instance.get("target").getAsString())
                : Target.LOCAL;

        List<Processor> pipeline = new ArrayList<>();
        if (instance.has("pipeline")) {
            for (JsonElement e : instance.getAsJsonArray("pipeline")) {
                pipeline.add(buildStage(e.getAsJsonObject()));
            }
        }

        int maxQueue = (int) resolve(instance, globalDefaults, "maxQueue", DEFAULT_MAX_QUEUE);
        long tickMs = resolve(instance, globalDefaults, "tickMs", DEFAULT_TICK_MS);

        return new RouteConfig(id, List.copyOf(subscribe), publishTopic, target,
                List.copyOf(pipeline), maxQueue, tickMs);
    }

    /**
     * Builds one stage from its single-key config object. <b>Add a case here as you add a stage</b>
     * (and a matching branch to {@code config.schema.json}'s {@code $defs/stage}).
     */
    private static Processor buildStage(JsonObject stage) {
        if (stage.size() != 1) {
            throw new IllegalArgumentException("a stage is a single-key object, got: " + stage);
        }
        String name = stage.keySet().iterator().next();
        JsonObject args = stage.getAsJsonObject(name);
        return switch (name) {
            case "fieldEquals" -> new Stages.FieldEquals(
                    requireString(args, "path"),
                    requirePresent(args, "value"));
            case "countPerTick" -> new Stages.CountPerTick();
            default -> throw new IllegalArgumentException("unknown pipeline stage `" + name + "`");
        };
    }

    /** Per-route value ▸ {@code component.global.defaults} ▸ the built-in default. */
    private static long resolve(JsonObject instance, JsonObject defaults, String key, long fallback) {
        if (instance.has(key)) {
            return instance.get(key).getAsLong();
        }
        if (defaults != null && defaults.has(key)) {
            return defaults.get(key).getAsLong();
        }
        return fallback;
    }

    private static String requireString(JsonObject o, String key) {
        return requirePresent(o, key).getAsString();
    }

    private static JsonElement requirePresent(JsonObject o, String key) {
        JsonElement e = o.get(key);
        if (e == null || e.isJsonNull()) {
            throw new IllegalArgumentException("route is missing the required key `" + key + "`");
        }
        return e;
    }
}
