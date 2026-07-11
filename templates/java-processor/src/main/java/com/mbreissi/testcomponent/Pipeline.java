package <<PACKAGE>>;

import java.util.ArrayList;
import java.util.List;

/**
 * An ordered chain of stages: the output of each is the input of the next.
 *
 * <p>A {@code Pipeline} is owned by exactly one route worker thread and is <b>not</b> thread-safe —
 * that is the point (see {@link Processor}).
 */
public final class Pipeline {

    private final List<Processor> stages;

    public Pipeline(List<Processor> stages) {
        this.stages = List.copyOf(stages);
    }

    /** The stages, in order. */
    public List<Processor> stages() {
        return stages;
    }

    /**
     * Runs a batch through every stage in order.
     *
     * <p>When {@code nowMs} is non-null, each stage additionally gets an
     * {@link Processor#onTick(long)} <i>after</i> its data pass, and whatever it emits joins the
     * batch flowing downstream — so a window closing in stage 1 is still projected by stage 2 on
     * the same pass, rather than waiting for the next message to shake it loose.
     *
     * @param input the messages entering the pipeline (empty on a pure tick)
     * @param nowMs the tick instant in epoch milliseconds, or {@code null} for a data-only pass
     * @return what came out the far end — zero, one, or many messages
     */
    public List<ProcMsg> run(List<ProcMsg> input, Long nowMs) {
        List<ProcMsg> carried = new ArrayList<>(input);
        for (Processor stage : stages) {
            List<ProcMsg> next = new ArrayList<>(carried.size());
            for (ProcMsg m : carried) {
                next.addAll(stage.process(m));
            }
            if (nowMs != null) {
                next.addAll(stage.onTick(nowMs));
            }
            carried = next;
        }
        return carried;
    }
}
