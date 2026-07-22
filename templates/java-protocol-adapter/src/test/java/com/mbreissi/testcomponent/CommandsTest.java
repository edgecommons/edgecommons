package <<PACKAGE>>;

import com.mbreissi.edgecommons.commands.CommandException;
import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import com.google.gson.JsonPrimitive;
import org.junit.jupiter.api.Test;

import java.util.ArrayList;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.junit.jupiter.api.Assertions.fail;

/**
 * Every {@code sb/*} verb's happy path + each error code + the single-instance default; the allow-list
 * refusal proven to happen BEFORE any device I/O; pause gating a poll; reconnect; and the panel
 * registration. A mock {@link Commands.DeviceControl} services the seam and RECORDS every write that
 * reaches it — no device, no socket.
 */
class CommandsTest {

    // --- a mock control seam that records the writes that reach the "device" -----------------------

    private enum Browse { ONE, UNSUPPORTED, FAILED }

    private static final class MockControl implements Commands.DeviceControl {
        final Health health;
        boolean writeOk = true;
        boolean readOk = true;
        boolean reconnectOk = true;
        boolean unavailable = false;
        Browse browse = Browse.ONE;
        /** Every write that REACHED the device — empty proves the allow-list refused before any I/O. */
        final List<String> writes = new ArrayList<>();

        MockControl(Health health) {
            this.health = health;
        }

        @Override
        public List<Device.Reading> readNow(List<String> ids)
                throws Commands.ReadFailedException, Commands.DeviceUnavailableException {
            if (unavailable) {
                throw Commands.DeviceUnavailableException.gone();
            }
            if (!readOk) {
                throw new Commands.ReadFailedException("link error");
            }
            List<Device.Reading> out = new ArrayList<>();
            for (String id : ids) {
                out.add(new Device.Reading(id, null, new JsonPrimitive(42.0), Device.Quality.GOOD, "OK"));
            }
            return out;
        }

        @Override
        public void write(String signalId, JsonElement value)
                throws Commands.WriteFailedException, Commands.DeviceUnavailableException {
            if (unavailable) {
                throw Commands.DeviceUnavailableException.gone();
            }
            writes.add(signalId);
            if (!writeOk) {
                throw new Commands.WriteFailedException("device rejected");
            }
        }

        @Override
        public Device.BrowsePage browse(String cursor, int max)
                throws Device.BrowseException, Commands.DeviceUnavailableException {
            if (unavailable) {
                throw Commands.DeviceUnavailableException.gone();
            }
            return switch (browse) {
                case ONE -> new Device.BrowsePage(
                        List.of(new Device.BrowsedSignal("temperature-1", "Ambient temperature", "REAL")),
                        null);
                case UNSUPPORTED -> throw Device.BrowseException.unsupported();
                case FAILED -> throw Device.BrowseException.failed("mid-browse error");
            };
        }

        @Override
        public boolean pause() {
            return Wiring.setPaused(health, true);
        }

        @Override
        public boolean resume() {
            return Wiring.setPaused(health, false);
        }

        @Override
        public void reconnect()
                throws Commands.ReconnectFailedException, Commands.DeviceUnavailableException {
            if (unavailable) {
                throw Commands.DeviceUnavailableException.gone();
            }
            if (!reconnectOk) {
                throw new Commands.ReconnectFailedException("no route to host");
            }
        }

        @Override
        public long repoll() throws Commands.DeviceUnavailableException {
            if (unavailable) {
                throw new Commands.DeviceUnavailableException("link error");
            }
            return 2;
        }
    }

    // --- fixtures ----------------------------------------------------------------------------------

    private static JsonObject json(String s) {
        return JsonParser.parseString(s).getAsJsonObject();
    }

    private static DeviceConfig aDevice() {
        return aDevice("plc-1");
    }

    private static DeviceConfig aDevice(String id) {
        return new DeviceConfig(id, "sim",
                new Device.ConnectionConfig("sim://" + id, new JsonObject()), 5_000L,
                new Writes(List.of("setpoint-1")));
    }

    private static List<Device.SignalInfo> simSignals() {
        return List.of(
                new Device.SignalInfo("temperature-1", "Ambient temperature"),
                new Device.SignalInfo("setpoint-1", "Setpoint"));
    }

    private static DeviceMetrics dm(DeviceConfig cfg, Health health) {
        // No emitter/config: the Commander touches only the counters (recordCommand / countersView),
        // never define/emit — so a metric-less DeviceMetrics is enough to exercise the command surface.
        return new DeviceMetrics(null, null, cfg.id(), health, 30);
    }

    private static final class Harness {
        final Commands.Commander commander;
        final MockControl control;
        final Health health;

        Harness(MockControl control, Health health, Commands.Commander commander) {
            this.control = control;
            this.health = health;
            this.commander = commander;
        }
    }

    private static Harness harness(DeviceConfig cfg) {
        Health health = new Health();
        health.setLink(LinkState.ONLINE);
        MockControl control = new MockControl(health);
        DeviceMetrics dm = dm(cfg, health);
        Commands.DeviceHandle handle = new Commands.DeviceHandle(cfg, control, health, dm, simSignals());
        return new Harness(control, health, new Commands.Commander(List.of(handle)));
    }

    @FunctionalInterface
    private interface Call {
        JsonObject run() throws CommandException;
    }

    private static String errCode(Call call) {
        try {
            call.run();
        } catch (CommandException e) {
            return e.getCode();
        }
        fail("command should have failed");
        return null;
    }

    // --- routing / single-instance default (D-EIP-13) ---------------------------------------------

    @Test
    void instanceDefaultsToTheSoleDeviceAndUnknownOrMissingIdsError() throws Exception {
        Harness h = harness(aDevice());
        assertEquals("plc-1", h.commander.status(json("{}")).get("id").getAsString());
        assertEquals("NO_SUCH_INSTANCE", errCode(() -> h.commander.status(json("{\"instance\":\"nope\"}"))));

        // Two devices: a missing `instance` is BAD_ARGS.
        Commands.DeviceHandle a = handleFor(aDevice("plc-1"));
        Commands.DeviceHandle b = handleFor(aDevice("plc-2"));
        Commands.Commander multi = new Commands.Commander(List.of(a, b));
        assertEquals("BAD_ARGS", errCode(() -> multi.status(json("{}"))));
        assertEquals("plc-2", multi.status(json("{\"instance\":\"plc-2\"}")).get("id").getAsString());
    }

    private static Commands.DeviceHandle handleFor(DeviceConfig cfg) {
        Health health = new Health();
        health.setLink(LinkState.ONLINE);
        return new Commands.DeviceHandle(cfg, new MockControl(health), health, dm(cfg, health), simSignals());
    }

    // --- sb/status ---------------------------------------------------------------------------------

    @Test
    void statusReportsConnectedStatePausedAndACounterSnapshot() throws Exception {
        Harness h = harness(aDevice());
        JsonObject out = h.commander.status(json("{}"));
        assertTrue(out.get("connected").getAsBoolean());
        assertEquals("ONLINE", out.get("state").getAsString());
        assertFalse(out.get("paused").getAsBoolean());
        assertEquals("sim", out.get("adapter").getAsString());
        assertTrue(out.getAsJsonObject("metrics").has("connectAttempts"));
    }

    // --- sb/signals --------------------------------------------------------------------------------

    @Test
    void signalsListsTheInventoryWithTheWritableFlag() throws Exception {
        Harness h = harness(aDevice());
        JsonArray sigs = h.commander.signals(json("{}")).getAsJsonArray("signals");
        assertEquals(2, sigs.size());
        JsonObject setpoint = findSignal(sigs, "setpoint-1");
        assertTrue(setpoint.get("writable").getAsBoolean(), "setpoint-1 is on the allow-list");
        JsonObject temp = findSignal(sigs, "temperature-1");
        assertFalse(temp.get("writable").getAsBoolean(), "temperature-1 is not");
    }

    private static JsonObject findSignal(JsonArray sigs, String id) {
        for (JsonElement e : sigs) {
            if (e.getAsJsonObject().get("id").getAsString().equals(id)) {
                return e.getAsJsonObject();
            }
        }
        throw new AssertionError("no signal " + id);
    }

    // --- sb/read -----------------------------------------------------------------------------------

    @Test
    void readReturnsValuesByIdAndByNameAndMarksUnresolvedRefs() throws Exception {
        Harness h = harness(aDevice());
        JsonObject out = h.commander.read(json(
                "{\"signals\":[{\"signalId\":\"temperature-1\"},{\"name\":\"Setpoint\"},{\"name\":\"ghost\"}]}"));
        JsonArray reads = out.getAsJsonArray("reads");
        assertEquals("temperature-1", reads.get(0).getAsJsonObject().getAsJsonObject("signal").get("id").getAsString());
        assertEquals("GOOD", reads.get(0).getAsJsonObject().get("quality").getAsString());
        assertEquals("setpoint-1", reads.get(1).getAsJsonObject().getAsJsonObject("signal").get("id").getAsString(),
                "resolved by name");
        assertEquals("BAD", reads.get(2).getAsJsonObject().get("quality").getAsString(),
                "an unknown name is a BAD/unresolved entry");
        assertEquals("UNRESOLVED_REF", reads.get(2).getAsJsonObject().get("qualityRaw").getAsString());
    }

    @Test
    void readWithoutASignalsArrayIsBadArgsAndALinkErrorIsReadFailed() {
        Harness h = harness(aDevice());
        assertEquals("BAD_ARGS", errCode(() -> h.commander.read(json("{}"))));

        Harness h2 = harness(aDevice());
        h2.control.readOk = false;
        assertEquals("READ_FAILED", errCode(
                () -> h2.commander.read(json("{\"signals\":[{\"signalId\":\"temperature-1\"}]}"))));
    }

    // --- sb/write: allow-list BEFORE any device I/O (the security guarantee) -----------------------

    @Test
    void aRefusedWriteNeverReachesTheDevice() {
        Harness h = harness(aDevice());
        // temperature-1 is NOT on the allow-list.
        assertEquals("WRITE_NOT_ALLOWED", errCode(() -> h.commander.write(
                json("{\"writes\":[{\"signalId\":\"temperature-1\",\"value\":1}]}"))));
        assertTrue(h.control.writes.isEmpty(), "the refused write must never reach the device");
    }

    @Test
    void anAllowListedWriteIsConfirmedAndBatchesMixResults() throws Exception {
        Harness h = harness(aDevice());
        // A single allowed write (single-object shorthand).
        JsonObject out = h.commander.write(json("{\"signalId\":\"setpoint-1\",\"value\":42}"));
        assertEquals(1, out.get("written").getAsInt());
        assertEquals(1, h.control.writes.size(), "the allowed write reached the device");

        // A batch: one allowed (written), one refused (never sent).
        JsonObject out2 = h.commander.write(json(
                "{\"writes\":[{\"signalId\":\"setpoint-1\",\"value\":7},{\"signalId\":\"temperature-1\",\"value\":8}]}"));
        assertEquals(1, out2.get("written").getAsInt(), "only the allow-listed entry is written");
        JsonArray results = out2.getAsJsonArray("results");
        long okCount = countWhere(results, r -> r.has("ok") && r.get("ok").getAsBoolean());
        long refusedCount = countWhere(results,
                r -> r.has("error") && r.get("error").getAsString().equals("not in writes.allow"));
        assertEquals(1, okCount);
        assertEquals(1, refusedCount);
        // Two device writes total (one from each successful call); the refused entry added none.
        assertEquals(2, h.control.writes.size());
    }

    private static long countWhere(JsonArray arr, java.util.function.Predicate<JsonObject> p) {
        long n = 0;
        for (JsonElement e : arr) {
            if (p.test(e.getAsJsonObject())) {
                n++;
            }
        }
        return n;
    }

    @Test
    void aWriteTheDeviceRejectsIsWriteFailed() {
        Harness h = harness(aDevice());
        h.control.writeOk = false;
        assertEquals("WRITE_FAILED",
                errCode(() -> h.commander.write(json("{\"signalId\":\"setpoint-1\",\"value\":42}"))));
    }

    @Test
    void aWriteWithNoWritesOrValueIsBadArgs() {
        Harness h = harness(aDevice());
        assertEquals("BAD_ARGS", errCode(() -> h.commander.write(json("{}"))));
    }

    // --- sb/browse ---------------------------------------------------------------------------------

    @Test
    void browseReturnsAPageOrTheRightErrorCode() throws Exception {
        Harness h = harness(aDevice());
        JsonObject out = h.commander.browse(json("{}"));
        assertEquals(1, out.getAsJsonArray("entries").size());
        assertEquals("temperature-1",
                out.getAsJsonArray("entries").get(0).getAsJsonObject().get("id").getAsString());

        Harness u = harness(aDevice());
        u.control.browse = Browse.UNSUPPORTED;
        assertEquals("BROWSE_UNSUPPORTED", errCode(() -> u.commander.browse(json("{}"))));

        Harness f = harness(aDevice());
        f.control.browse = Browse.FAILED;
        assertEquals("BROWSE_FAILED", errCode(() -> f.commander.browse(json("{}"))));
    }

    // --- pause / resume / repoll -------------------------------------------------------------------

    @Test
    void pauseIsIdempotentAndRepollIsRefusedWhilePaused() throws Exception {
        Harness h = harness(aDevice());

        // repoll works while running.
        assertEquals(2, h.commander.repoll(json("{}")).get("polled").getAsInt());

        JsonObject out = h.commander.pause(json("{}"));
        assertTrue(out.get("paused").getAsBoolean());
        assertTrue(out.get("changed").getAsBoolean());
        assertTrue(h.health.isPaused());

        // repoll is refused while paused (BAD_ARGS).
        assertEquals("BAD_ARGS", errCode(() -> h.commander.repoll(json("{}"))));

        // pausing again is idempotent.
        assertFalse(h.commander.pause(json("{}")).get("changed").getAsBoolean());

        // resume clears it and repoll works again.
        JsonObject resumed = h.commander.resume(json("{}"));
        assertFalse(resumed.get("paused").getAsBoolean());
        assertTrue(resumed.get("changed").getAsBoolean());
        assertFalse(h.health.isPaused());
        assertEquals(2, h.commander.repoll(json("{}")).get("polled").getAsInt());
    }

    // --- reconnect ---------------------------------------------------------------------------------

    @Test
    void reconnectConfirmsOrReportsReconnectFailed() throws Exception {
        Harness h = harness(aDevice());
        assertTrue(h.commander.reconnect(json("{}")).get("connected").getAsBoolean());

        Harness f = harness(aDevice());
        f.control.reconnectOk = false;
        assertEquals("RECONNECT_FAILED", errCode(() -> f.commander.reconnect(json("{}"))));
    }

    @Test
    void deviceUnavailableWhenTheTaskIsGone() {
        Harness h = harness(aDevice());
        h.control.unavailable = true;
        assertEquals("DEVICE_UNAVAILABLE", errCode(() -> h.commander.reconnect(json("{}"))));
    }

    // --- panels ------------------------------------------------------------------------------------

    @Test
    void theThreePanelsAreRegisteredWithTheRightIdsOrdersAndScope() {
        List<JsonObject> ps = Commands.panels();
        List<String> ids = ps.stream().map(p -> p.get("id").getAsString()).toList();
        assertEquals(List.of("overview", "signals", "diagnostics"), ids);
        List<Integer> orders = ps.stream().map(p -> p.get("order").getAsInt()).toList();
        assertEquals(List.of(10, 20, 30), orders);
        for (JsonObject p : ps) {
            assertEquals("instance", p.get("scope").getAsString(), "every panel is instance-scoped");
        }
        // The signals panel binds the signal verbs; diagnostics binds browse.
        assertEquals(List.of("sb/signals", "sb/read", "sb/write", "repoll"),
                verbsOf(ps.get(1)));
        assertEquals(List.of("sb/browse", "sb/status"), verbsOf(ps.get(2)));
        // Pause/resume are bound on the overview panel (SD-2: included in the templates).
        assertTrue(verbsOf(ps.get(0)).contains("sb/pause"));
        assertTrue(verbsOf(ps.get(0)).contains("sb/resume"));
        assertNull(ps.get(0).get("nonexistent"));
    }

    private static List<String> verbsOf(JsonObject panel) {
        List<String> out = new ArrayList<>();
        for (JsonElement e : panel.getAsJsonArray("verbs")) {
            out.add(e.getAsString());
        }
        return out;
    }
}
