package <<PACKAGE>>;

import com.google.gson.JsonParser;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import java.util.stream.Stream;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

/** The destination contract: a stable key, an idempotent overwrite, and a verify that means it. */
class LocalDestinationTest {

    private static Item item(String key, String body) {
        return new Item(key, body.getBytes(StandardCharsets.UTF_8));
    }

    private static List<Path> listing(Path dir) throws IOException {
        try (Stream<Path> s = Files.list(dir)) {
            return s.toList();
        }
    }

    @Test
    void deliveryLandsTheObjectAtItsStableKey(@TempDir Path dir) throws Exception {
        LocalDestination dest = new LocalDestination(dir);

        Item it = item("a/b/thing.json", "hello");
        Delivered got = dest.deliver(it);
        assertEquals(5, got.bytesWritten());
        dest.verify(it, got); // does not throw

        assertEquals("hello", Files.readString(dir.resolve("a/b/thing.json")));
    }

    @Test
    void redeliveryOverwritesRatherThanDuplicating(@TempDir Path dir) throws Exception {
        // This is what makes retry safe. If a redelivery could duplicate, a sink could not retry.
        LocalDestination dest = new LocalDestination(dir);

        dest.deliver(item("thing.json", "first"));
        Item second = item("thing.json", "second");
        Delivered got = dest.deliver(second);
        dest.verify(second, got);

        assertEquals("second", Files.readString(dir.resolve("thing.json")));
        assertEquals(1, listing(dir).size(), "one object, not two");
    }

    @Test
    void noPartialFileIsLeftBehind(@TempDir Path dir) throws Exception {
        // The temp file must be RENAMED into place — atomically — not copied and left behind. A
        // reader must never observe a half-written object at the real key.
        new LocalDestination(dir).deliver(item("thing.json", "hello"));

        assertFalse(listing(dir).stream().anyMatch(p -> p.getFileName().toString().contains("partial")),
                "the temp file must be renamed, not left");
    }

    @Test
    void verifyRefusesAMismatchSoTheSourceIsNeverReleased(@TempDir Path dir) throws Exception {
        LocalDestination dest = new LocalDestination(dir);
        Item it = item("thing.json", "hello");
        dest.deliver(it);

        // Claim we wrote more than we did: verify must catch it, and must not report success.
        DeliverException e = assertThrows(DeliverException.class,
                () -> dest.verify(it, new Delivered(999)));
        assertTrue(e.isTransient(), "a mismatch is worth another attempt, not a silent loss");
        assertTrue(e.getMessage().contains("size mismatch"), e.getMessage());
    }

    @Test
    void verifyRefusesAnObjectThatIsNotThereAtAll(@TempDir Path dir) {
        LocalDestination dest = new LocalDestination(dir);
        assertThrows(DeliverException.class,
                () -> dest.verify(item("never-written.json", ""), new Delivered(0)));
    }

    @Test
    void errorClassificationDecidesWhetherRetryingCanHelp() {
        assertTrue(DeliverException.transientFailure("timeout").isTransient());
        assertFalse(DeliverException.permanentFailure("bad credentials").isTransient());
    }

    @Test
    void aDestinationIsBuiltFromConfig(@TempDir Path dir) {
        Destination d = Destination.build(JsonParser
                .parseString("{\"type\":\"local\",\"path\":\"" + dir.toString().replace("\\", "\\\\") + "\"}")
                .getAsJsonObject());
        assertEquals("local", d.kind());
    }

    @Test
    void anUnknownDestinationTypeIsRejectedRatherThanIgnored() {
        assertThrows(IllegalArgumentException.class, () -> Destination.build(
                JsonParser.parseString("{\"type\":\"s3\",\"path\":\"x\"}").getAsJsonObject()));
    }
}
