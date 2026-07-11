package <<PACKAGE>>;

import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.mbreissi.edgecommons.messaging.Message;
import com.mbreissi.edgecommons.messaging.MessageBuilder;

import java.util.List;

/**
 * The demo stages, and the one helper they share.
 *
 * <p>Two stages, enough to show both halves of {@link Processor}: a stateless filter that emits on
 * arrival, and a stateful rollup that emits on the tick. Replace them with your own — nothing here
 * is required by the library.
 */
public final class Stages {

    private Stages() {
    }

    /**
     * Resolves a dotted path ({@code signal.id}) inside a message body.
     *
     * @param body the message body (only a {@link JsonObject} body is walkable)
     * @param path the dotted path
     * @return the element at that path, or {@code null} if any segment is missing
     */
    public static JsonElement pluck(Object body, String path) {
        if (!(body instanceof JsonObject obj)) {
            return null;
        }
        JsonElement cursor = obj;
        for (String segment : path.split("\\.")) {
            if (!(cursor instanceof JsonObject o)) {
                return null;
            }
            cursor = o.get(segment);
            if (cursor == null) {
                return null;
            }
        }
        return cursor;
    }

    /**
     * Drops any message whose dotted body path does not equal an expected value.
     *
     * <p>A filter is the simplest useful stage: it returns nothing, and the message stops there.
     */
    public static final class FieldEquals implements Processor {

        private final String path;
        private final JsonElement value;

        public FieldEquals(String path, JsonElement value) {
            this.path = path;
            this.value = value;
        }

        @Override
        public List<ProcMsg> process(ProcMsg m) {
            JsonElement found = pluck(m.msg().getBody(), path);
            return value.equals(found) ? List.of(m) : List.of();
        }
    }

    /**
     * Counts messages and emits a rollup on each tick.
     *
     * <p>This is the stateful half of the interface: it accumulates in
     * {@link #process(ProcMsg)} (emitting nothing) and produces its output in
     * {@link #onTick(long)}. Windows, batches and debounces are all this shape.
     *
     * <p>The fields are plain, unsynchronized state — legal precisely because a route's pipeline is
     * owned by one worker thread (see {@link Processor}).
     */
    public static final class CountPerTick implements Processor {

        private long seen;
        private ProcMsg last;

        @Override
        public List<ProcMsg> process(ProcMsg m) {
            seen++;
            last = m;
            return List.of(); // nothing goes downstream on arrival — see onTick
        }

        @Override
        public List<ProcMsg> onTick(long nowMs) {
            if (seen == 0 || last == null) {
                return List.of(); // an empty window is not an event
            }
            JsonObject body = new JsonObject();
            body.addProperty("count", seen);
            body.add("last", asJson(last.msg().getBody()));

            Message source = last.msg();
            Message rolled = MessageBuilder.create(source.getHeader().getName(), source.getHeader().getVersion())
                    .withPayload(body)
                    .build();
            ProcMsg out = last.withMessage(rolled);

            seen = 0;
            last = null;
            return List.of(out);
        }

        /** A non-JSON body (an opaque/binary payload) has nothing to nest; carry an empty object. */
        private static JsonElement asJson(Object body) {
            return body instanceof JsonElement e ? e : new JsonObject();
        }
    }
}
