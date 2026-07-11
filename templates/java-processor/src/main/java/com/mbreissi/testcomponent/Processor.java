package <<PACKAGE>>;

import java.util.List;

/**
 * One stage of the pipeline. <b>This is the interface you implement.</b>
 *
 * <h2>Why a stage returns a {@code List} and not an {@code Optional}</h2>
 *
 * <p>A filter drops, a projection maps, an aggregator emits several. <b>0..N</b> covers all three
 * without a special case, and it is what lets {@link #onTick(long)} exist: a <i>stateful</i> stage
 * (a window, a debounce, a batch) accumulates in {@link #process(ProcMsg)} and emits in
 * {@link #onTick(long)}, so time-driven output is not a different mechanism from data-driven
 * output.
 *
 * <h2>One worker per route, so state needs no lock</h2>
 *
 * <p>Each route owns its {@link Pipeline} on a single thread. That is deliberate: per-key state
 * inside a stage is a plain field with no synchronization anywhere, which is what makes a stateful
 * stage cheap to write correctly. A stage is therefore <b>not</b> required to be thread-safe.
 */
public interface Processor {

    /**
     * Handles one inbound message. Returns what should continue downstream: nothing (a filter that
     * dropped it), one message (a map), or several (a fan-out).
     *
     * @param m the message and the topic it arrived on
     * @return zero or more messages for the next stage
     */
    List<ProcMsg> process(ProcMsg m);

    /**
     * Called periodically, for stages that emit on time rather than on arrival (a window, a batch,
     * a debounce). The default emits nothing — a stateless stage ignores time.
     *
     * @param nowMs the wall-clock tick, in epoch milliseconds
     * @return zero or more messages for the next stage
     */
    default List<ProcMsg> onTick(long nowMs) {
        return List.of();
    }
}
