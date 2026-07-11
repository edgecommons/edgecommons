package <<PACKAGE>>;

import com.google.gson.JsonObject;
import com.mbreissi.edgecommons.messaging.Message;
import com.mbreissi.edgecommons.messaging.MessageBuilder;
import com.mbreissi.edgecommons.messaging.MessageIdentity;
import org.junit.jupiter.api.Test;

import java.util.List;

import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * The self-echo guard.
 *
 * <p>A processor that publishes onto a class it also subscribes to will consume its own output,
 * reprocess it, republish it, and saturate the device. This is the check that stops it, and it is
 * why the identity restamp in {@code dispatch} is not decoration: the guard has nothing to compare
 * against if we forward a producer's identity as if it were our own.
 */
class <<COMPONENTNAME>>Test {

    private static final String MY_PATH = "factory-1/gw-01";
    private static final String MY_COMPONENT = "proc";

    private static MessageIdentity identity(String site, String device, String component) {
        return new MessageIdentity(
                List.of(new MessageIdentity.HierEntry("site", site),
                        new MessageIdentity.HierEntry("device", device)),
                component, "main");
    }

    private static Message from(MessageIdentity id) {
        return MessageBuilder.create("T", "1.0")
                .withPayload(new JsonObject())
                .withIdentity(id)
                .build();
    }

    @Test
    void ourOwnOutputIsDroppedRatherThanReprocessedForever() {
        Message mine = from(identity("factory-1", "gw-01", MY_COMPONENT));
        assertTrue(<<COMPONENTNAME>>.isSelfEcho(mine, MY_PATH, MY_COMPONENT));
    }

    @Test
    void anotherComponentOnThisDeviceIsNotAnEcho() {
        Message theirs = from(identity("factory-1", "gw-01", "modbus-adapter"));
        assertFalse(<<COMPONENTNAME>>.isSelfEcho(theirs, MY_PATH, MY_COMPONENT),
                "same device, different component — this is exactly the traffic we exist to process");
    }

    @Test
    void thisSameComponentOnAnotherDeviceIsNotAnEcho() {
        Message elsewhere = from(identity("factory-1", "gw-02", MY_COMPONENT));
        assertFalse(<<COMPONENTNAME>>.isSelfEcho(elsewhere, MY_PATH, MY_COMPONENT),
                "our sibling on another device is a peer, not our own echo");
    }

    @Test
    void anUnstampedMessageIsNotAnEcho() {
        // An identity-free envelope (a raw/bootstrap publish) cannot be ours.
        Message anonymous = MessageBuilder.create("T", "1.0").withPayload(new JsonObject()).build();
        assertFalse(<<COMPONENTNAME>>.isSelfEcho(anonymous, MY_PATH, MY_COMPONENT));
    }
}
