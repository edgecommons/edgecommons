package <<PACKAGE>>;

import com.mbreissi.edgecommons.messaging.Message;

/**
 * A message in flight, and the topic it arrived on.
 *
 * <p>The topic is carried because a stage may want to route on it, and because the dispatcher needs
 * it to decide where the result goes.
 *
 * @param topic the topic it arrived on — the demo stages ignore it; yours may want to route on it
 * @param msg   the message itself
 */
public record ProcMsg(String topic, Message msg) {

    /** Returns a copy of this unit carrying a different message on the same topic. */
    public ProcMsg withMessage(Message replacement) {
        return new ProcMsg(topic, replacement);
    }
}
