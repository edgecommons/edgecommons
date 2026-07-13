package <<PACKAGE>>;

import com.google.gson.JsonObject;
import com.mbreissi.edgecommons.heartbeat.InstanceConnectivity;
import com.mbreissi.edgecommons.messaging.Message;
import com.mbreissi.edgecommons.messaging.MessageBuilder;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.stream.Stream;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * The stable key.
 *
 * <p>A sink's ability to retry rests entirely on this: the same message must always resolve to the
 * same destination key, so a redelivery <b>overwrites</b>. A key derived from the clock, a counter,
 * or a random id would turn every retry into a duplicate — and a sink that cannot retry without
 * duplicating cannot retry at all.
 */
class <<COMPONENTNAME>>Test {

    private static Message message() {
        return MessageBuilder.create("SouthboundSignalUpdate", "1.0")
                .withPayload(new JsonObject())
                .build();
    }

    @Test
    void theKeyIsDeterministic() {
        Message msg = message();
        String a = <<COMPONENTNAME>>.keyFor("archive", "ecv1/gw/x/main/data/temp", msg);
        String b = <<COMPONENTNAME>>.keyFor("archive", "ecv1/gw/x/main/data/temp", msg);

        assertEquals(a, b, "the same message must always resolve to the same key");
        assertTrue(a.startsWith("archive/temp/"), a);
        assertTrue(a.endsWith(".json"), a);
    }

    @Test
    void twoMessagesDoNotCollide() {
        assertNotEquals(
                <<COMPONENTNAME>>.keyFor("archive", "ecv1/gw/x/main/data/temp", message()),
                <<COMPONENTNAME>>.keyFor("archive", "ecv1/gw/x/main/data/temp", message()),
                "distinct envelopes must not overwrite each other");
    }

    @Test
    void theSinkIdPrefixesTheKeySoTwoSinksNeverCollide() {
        Message msg = message();
        assertTrue(<<COMPONENTNAME>>.keyFor("archive", "ecv1/gw/x/main/data/t", msg).startsWith("archive/"));
        assertTrue(<<COMPONENTNAME>>.keyFor("cold-store", "ecv1/gw/x/main/data/t", msg).startsWith("cold-store/"));
    }

    @Test
    void aRedeliveryToTheStableKeyLeavesExactlyOneObject(@TempDir Path dir) throws Exception {
        // The whole retry story, end to end: deliver the same message twice — as a retry would —
        // and the destination must hold ONE object, not two.
        LocalDestination dest = new LocalDestination(dir);
        Message msg = message();
        String key = <<COMPONENTNAME>>.keyFor("archive", "ecv1/gw/x/main/data/temp", msg);

        Item item = new Item(key, "{\"v\":1}".getBytes(StandardCharsets.UTF_8));
        dest.verify(item, dest.deliver(item));
        dest.verify(item, dest.deliver(item)); // the redelivery

        assertEquals(1, count(dir.resolve("archive/temp")));
    }

    private static long count(Path dir) throws IOException {
        try (Stream<Path> s = Files.list(dir)) {
            return s.count();
        }
    }

    @Test
    void aReachableDestinationIsReportedAsAnInstanceOfThisComponent() {
        // A sink's destinations ARE its instances: this is what the `state` keepalive pushes and
        // what the built-in `status` verb answers with.
        InstanceConnectivity up = <<COMPONENTNAME>>.connectivity("archive", "local", true, "ONLINE", null);

        assertEquals("archive", up.getInstance());
        assertTrue(up.isConnected(), "connected is the NORMALIZED flag every console reads");
        assertEquals("ONLINE", up.getState());
        assertEquals("local", up.getAttributes().get("kind").getAsString(),
                "domain data belongs in the open attributes bag, never in the normalized fields");
    }

    @Test
    void retryingAndGivingUpAreBothDisconnectedButAnOperatorMustTellThemApart() {
        // The reason `state` exists: a boolean cannot distinguish "still trying, the data is in
        // hand" from "gave up, the data did not arrive". Both are connected=false.
        InstanceConnectivity retrying = <<COMPONENTNAME>>.connectivity("archive", "local", false, "BACKOFF", "timeout");
        InstanceConnectivity gaveUp = <<COMPONENTNAME>>.connectivity("archive", "local", false, "FAILED", "no such bucket");

        assertFalse(retrying.isConnected());
        assertFalse(gaveUp.isConnected());
        assertNotEquals(retrying.getState(), gaveUp.getState());
        assertEquals("timeout", retrying.getDetail(), "the detail says WHY it is down");
    }
}
