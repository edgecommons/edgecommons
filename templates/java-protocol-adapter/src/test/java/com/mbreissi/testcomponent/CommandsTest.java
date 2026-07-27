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

    private enum Browse { ONE, MANY, UNSUPPORTED, FAILED }

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
                case MANY -> new Device.BrowsePage(
                        List.of(new Device.BrowsedSignal("temperature-1", "Ambient temperature", "REAL"),
                                new Device.BrowsedSignal("pressure-1", "Line pressure", "REAL"),
                                new Device.BrowsedSignal("setpoint-1", "Setpoint", "REAL")),
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
        return new DeviceMetrics(null, null, cfg.id(), health, 30, simSignals().size());
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
        assertEquals("plc-1", h.commander.status(null).get("id").getAsString());
        assertEquals("NO_SUCH_INSTANCE", errCode(() -> h.commander.status("nope")));

        // Two devices: a missing `instance` is BAD_ARGS.
        Commands.DeviceHandle a = handleFor(aDevice("plc-1"));
        Commands.DeviceHandle b = handleFor(aDevice("plc-2"));
        Commands.Commander multi = new Commands.Commander(List.of(a, b));
        assertEquals("BAD_ARGS", errCode(() -> multi.status(null)));
        assertEquals("plc-2", multi.status("plc-2").get("id").getAsString());
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
        JsonObject out = h.commander.status(null);
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
        JsonArray sigs = h.commander.signals(null).getAsJsonArray("signals");
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
                "{\"signals\":[{\"signalId\":\"temperature-1\"},{\"name\":\"Setpoint\"},{\"name\":\"ghost\"}]}"), null);
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
        assertEquals("BAD_ARGS", errCode(() -> h.commander.read(json("{}"), null)));

        Harness h2 = harness(aDevice());
        h2.control.readOk = false;
        assertEquals("READ_FAILED", errCode(
                () -> h2.commander.read(json("{\"signals\":[{\"signalId\":\"temperature-1\"}]}"), null)));
    }

    // --- sb/write: allow-list BEFORE any device I/O (the security guarantee) -----------------------

    @Test
    void aRefusedWriteNeverReachesTheDevice() {
        Harness h = harness(aDevice());
        // temperature-1 is NOT on the allow-list.
        assertEquals("WRITE_NOT_ALLOWED", errCode(() -> h.commander.write(
                json("{\"writes\":[{\"signalId\":\"temperature-1\",\"value\":1}]}"), null)));
        assertTrue(h.control.writes.isEmpty(), "the refused write must never reach the device");
        assertEquals(0, h.health.takeWriteErrors(),
                "an allow-list refusal is a policy error, not a device write error");
    }

    @Test
    void anAllowListedWriteIsConfirmedAndBatchesMixResults() throws Exception {
        Harness h = harness(aDevice());
        // A single allowed write (single-object shorthand).
        JsonObject out = h.commander.write(json("{\"signalId\":\"setpoint-1\",\"value\":42}"), null);
        assertEquals(1, out.get("written").getAsInt());
        assertEquals(1, h.control.writes.size(), "the allowed write reached the device");

        // A batch: one allowed (written), one refused (never sent).
        JsonObject out2 = h.commander.write(json(
                "{\"writes\":[{\"signalId\":\"setpoint-1\",\"value\":7},{\"signalId\":\"temperature-1\",\"value\":8}]}"), null);
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
    void aWriteTheDeviceRejectsIsWriteFailedAndCountsAWriteError() {
        Harness h = harness(aDevice());
        h.control.writeOk = false;
        assertEquals("WRITE_FAILED",
                errCode(() -> h.commander.write(json("{\"signalId\":\"setpoint-1\",\"value\":42}"), null)));
        assertEquals(1, h.health.takeWriteErrors(), "the device rejection feeds the writeErrors counter");
    }

    @Test
    void writeErrorsCountsOnlyDevicePathFailures() throws Exception {
        // A successful write counts nothing.
        Harness h = harness(aDevice());
        h.commander.write(json("{\"signalId\":\"setpoint-1\",\"value\":42}"), null);
        assertEquals(0, h.health.takeWriteErrors());

        // Unresolved refs and missing values are caller errors — no increment.
        h.commander.write(json(
                "{\"writes\":[{\"name\":\"ghost\",\"value\":1},{\"signalId\":\"setpoint-1\"},"
                        + "{\"signalId\":\"setpoint-1\",\"value\":2}]}"), null);
        assertEquals(0, h.health.takeWriteErrors());

        // A gone device loop mid-write IS a device-path failure.
        Harness gone = harness(aDevice());
        gone.control.unavailable = true;
        assertEquals("DEVICE_UNAVAILABLE",
                errCode(() -> gone.commander.write(json("{\"signalId\":\"setpoint-1\",\"value\":42}"), null)));
        assertEquals(1, gone.health.takeWriteErrors());
    }

    @Test
    void aWriteWithNoWritesOrValueIsBadArgs() {
        Harness h = harness(aDevice());
        assertEquals("BAD_ARGS", errCode(() -> h.commander.write(json("{}"), null)));
    }

    // --- sb/browse ---------------------------------------------------------------------------------

    @Test
    void browseReturnsAPageOrTheRightErrorCode() throws Exception {
        Harness h = harness(aDevice());
        JsonObject out = h.commander.browse(json("{}"), null);
        assertEquals(1, out.getAsJsonArray("entries").size());
        assertEquals("temperature-1",
                out.getAsJsonArray("entries").get(0).getAsJsonObject().get("id").getAsString());

        Harness u = harness(aDevice());
        u.control.browse = Browse.UNSUPPORTED;
        assertEquals("BROWSE_UNSUPPORTED", errCode(() -> u.commander.browse(json("{}"), null)));

        Harness f = harness(aDevice());
        f.control.browse = Browse.FAILED;
        assertEquals("BROWSE_FAILED", errCode(() -> f.commander.browse(json("{}"), null)));
    }

    // --- sb/browse: the hierarchical panel mode ----------------------------------------------------

    @Test
    void hierarchicalRootAnswersTheDeviceNodeOverTheSameInventory() throws Exception {
        Harness h = harness(aDevice());
        JsonObject out = h.commander.browse(json("{\"ref\":\"root\"}"), null);

        assertEquals("hierarchical", out.get("mode").getAsString());
        assertEquals(1, out.get("refCount").getAsInt());
        assertEquals(1, out.get("depth").getAsInt());
        assertFalse(out.get("truncated").getAsBoolean());

        JsonObject root = out.getAsJsonObject("root");
        assertEquals("root", root.get("nodeId").getAsString());
        assertEquals("plc-1", root.get("name").getAsString(), "the root node is named from the instance");
        assertEquals("device", root.get("nodeClass").getAsString());
        assertTrue(root.get("dataType").isJsonNull());

        JsonObject ref = root.getAsJsonArray("refs").get(0).getAsJsonObject();
        assertEquals("contains", ref.get("referenceType").getAsString());
        JsonObject target = ref.getAsJsonObject("target");
        assertEquals("temperature-1", target.get("nodeId").getAsString());
        assertEquals("Ambient temperature", target.get("name").getAsString());
        assertEquals("signal", target.get("nodeClass").getAsString());
        assertEquals("REAL", target.get("dataType").getAsString());
    }

    @Test
    void hierarchicalSignalRefIsAKnownLeafAndAnUnknownRefIsBadArgs() throws Exception {
        Harness h = harness(aDevice());
        JsonObject out = h.commander.browse(json("{\"ref\":\"temperature-1\"}"), null);
        JsonObject root = out.getAsJsonObject("root");
        assertEquals("temperature-1", root.get("nodeId").getAsString());
        assertEquals("signal", root.get("nodeClass").getAsString());
        assertEquals("REAL", root.get("dataType").getAsString());
        assertEquals(0, root.getAsJsonArray("refs").size(), "a known leaf answers refs: []");
        assertEquals(0, out.get("refCount").getAsInt());
        assertFalse(out.get("truncated").getAsBoolean());

        assertEquals("BAD_ARGS", errCode(() -> h.commander.browse(json("{\"ref\":\"ghost\"}"), null)));
    }

    @Test
    void mixingHierarchicalAndPagedArgsIsBadArgs() {
        Harness h = harness(aDevice());
        assertEquals("BAD_ARGS",
                errCode(() -> h.commander.browse(json("{\"ref\":\"root\",\"cursor\":\"x\"}"), null)));
        assertEquals("BAD_ARGS",
                errCode(() -> h.commander.browse(json("{\"depth\":2,\"max\":10}"), null)));
        // The hierarchical companions without `ref` are also refused: nothing to anchor the tree on.
        assertEquals("BAD_ARGS", errCode(() -> h.commander.browse(json("{\"depth\":2}"), null)));
    }

    @Test
    void hierarchicalDepthAndMaxRefsAreBoundedAndTruncationIsReported() throws Exception {
        // depth clamps into 1..4; maxRefs into 1..1000.
        Harness h = harness(aDevice());
        assertEquals(4, h.commander.browse(json("{\"ref\":\"root\",\"depth\":99}"), null)
                .get("depth").getAsInt());
        assertEquals(1, h.commander.browse(json("{\"ref\":\"root\",\"depth\":0}"), null)
                .get("depth").getAsInt());

        // Three browsable signals, maxRefs 2 -> two refs, truncated.
        Harness many = harness(aDevice());
        many.control.browse = Browse.MANY;
        JsonObject out = many.commander.browse(json("{\"ref\":\"root\",\"maxRefs\":2}"), null);
        assertEquals(2, out.getAsJsonObject("root").getAsJsonArray("refs").size());
        assertEquals(2, out.get("refCount").getAsInt());
        assertTrue(out.get("truncated").getAsBoolean());
    }

    @Test
    void hierarchicalBrowseMapsTheBrowseErrorCodesLikeThePagedMode() {
        Harness u = harness(aDevice());
        u.control.browse = Browse.UNSUPPORTED;
        assertEquals("BROWSE_UNSUPPORTED", errCode(() -> u.commander.browse(json("{\"ref\":\"root\"}"), null)));

        Harness f = harness(aDevice());
        f.control.browse = Browse.FAILED;
        assertEquals("BROWSE_FAILED", errCode(() -> f.commander.browse(json("{\"ref\":\"root\"}"), null)));
    }

    // --- pause / resume / repoll -------------------------------------------------------------------

    @Test
    void pauseIsIdempotentAndRepollIsRefusedWhilePaused() throws Exception {
        Harness h = harness(aDevice());

        // repoll works while running.
        assertEquals(2, h.commander.repoll(null).get("polled").getAsInt());

        JsonObject out = h.commander.pause(null);
        assertTrue(out.get("paused").getAsBoolean());
        assertTrue(out.get("changed").getAsBoolean());
        assertTrue(h.health.isPaused());

        // repoll is refused while paused, with the dedicated PAUSED code.
        assertEquals("PAUSED", errCode(() -> h.commander.repoll(null)));

        // pausing again is idempotent.
        assertFalse(h.commander.pause(null).get("changed").getAsBoolean());

        // resume clears it and repoll works again.
        JsonObject resumed = h.commander.resume(null);
        assertFalse(resumed.get("paused").getAsBoolean());
        assertTrue(resumed.get("changed").getAsBoolean());
        assertFalse(h.health.isPaused());
        assertEquals(2, h.commander.repoll(null).get("polled").getAsInt());
    }

    // --- reconnect ---------------------------------------------------------------------------------

    @Test
    void reconnectConfirmsOrReportsReconnectFailed() throws Exception {
        Harness h = harness(aDevice());
        assertTrue(h.commander.reconnect(null).get("connected").getAsBoolean());

        Harness f = harness(aDevice());
        f.control.reconnectOk = false;
        assertEquals("RECONNECT_FAILED", errCode(() -> f.commander.reconnect(null)));
    }

    @Test
    void deviceUnavailableWhenTheTaskIsGone() {
        Harness h = harness(aDevice());
        h.control.unavailable = true;
        assertEquals("DEVICE_UNAVAILABLE", errCode(() -> h.commander.reconnect(null)));
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

    /** The widget descriptors are exactly what the edge-console descriptor renderer reads. */
    @Test
    void thePanelWidgetsMatchTheRenderableDescriptorShapeExactly() {
        List<JsonObject> ps = Commands.panels();

        // overview: a summary with rows + a lifecycle commandSummary with verbs.
        JsonArray overviewWidgets = ps.get(0).getAsJsonArray("widgets");
        assertEquals(2, overviewWidgets.size());
        JsonObject summary = overviewWidgets.get(0).getAsJsonObject();
        assertEquals("summary", summary.get("kind").getAsString());
        assertEquals("overview-summary", summary.get("id").getAsString());
        assertEquals("Adapter overview", summary.get("title").getAsString());
        JsonArray rows = summary.getAsJsonArray("rows");
        assertEquals(3, rows.size());
        assertEquals("Signals", rows.get(0).getAsJsonObject().get("label").getAsString());
        assertEquals("Configured signal inventory via cmd/sb/signals",
                rows.get(0).getAsJsonObject().get("value").getAsString());
        assertNull(summary.get("fields"), "the renderer reads rows, not fields");
        JsonObject lifecycle = overviewWidgets.get(1).getAsJsonObject();
        assertEquals("commandSummary", lifecycle.get("kind").getAsString());
        assertEquals("overview-lifecycle", lifecycle.get("id").getAsString());
        assertEquals("Lifecycle bindings", lifecycle.get("title").getAsString());
        assertEquals(List.of("sb/status", "reconnect", "sb/pause", "sb/resume", "repoll"),
                strings(lifecycle.getAsJsonArray("verbs")));
        assertNull(lifecycle.get("actions"), "the renderer reads verbs, not actions");

        // signals: one signalGrid bound to sb/signals (with the subscriptionsVerb compat alias).
        JsonArray signalsWidgets = ps.get(1).getAsJsonArray("widgets");
        assertEquals(1, signalsWidgets.size());
        JsonObject grid = signalsWidgets.get(0).getAsJsonObject();
        assertEquals("signalGrid", grid.get("kind").getAsString());
        assertEquals("configured-signals", grid.get("id").getAsString());
        assertEquals("Configured signals", grid.get("title").getAsString());
        assertEquals("instance", grid.get("scope").getAsString(),
                "command-backed widgets repeat the instance scope");
        assertEquals("sb/signals", grid.get("signalsVerb").getAsString());
        assertEquals("sb/signals", grid.get("subscriptionsVerb").getAsString(),
                "the renderer-compat alias points at the same sb/signals verb");
        assertEquals("sb/read", grid.get("readVerb").getAsString());

        // diagnostics: the hierarchical treeBrowser + a diagnostic commandSummary.
        JsonArray diagWidgets = ps.get(2).getAsJsonArray("widgets");
        assertEquals(2, diagWidgets.size());
        JsonObject tree = diagWidgets.get(0).getAsJsonObject();
        assertEquals("treeBrowser", tree.get("kind").getAsString());
        assertEquals("inventory-tree", tree.get("id").getAsString());
        assertEquals("Inventory", tree.get("title").getAsString());
        assertEquals("instance", tree.get("scope").getAsString());
        assertEquals("hierarchical", tree.get("mode").getAsString());
        assertEquals("root", tree.get("rootRef").getAsString());
        assertEquals(1, tree.get("depth").getAsInt());
        assertEquals(200, tree.get("maxRefs").getAsInt());
        assertEquals("sb/browse", tree.get("browseVerb").getAsString());
        assertEquals("sb/read", tree.get("readVerb").getAsString());
        assertNull(tree.get("writeVerb"), "no writeVerb anywhere - the console has no write surface");
        JsonObject diagCommands = diagWidgets.get(1).getAsJsonObject();
        assertEquals("commandSummary", diagCommands.get("kind").getAsString());
        assertEquals("diagnostic-commands", diagCommands.get("id").getAsString());
        assertEquals("Diagnostic commands", diagCommands.get("title").getAsString());
        assertEquals(List.of("sb/status", "sb/browse"), strings(diagCommands.getAsJsonArray("verbs")));

        // No widget anywhere carries a writeVerb.
        for (JsonObject p : ps) {
            for (JsonElement w : p.getAsJsonArray("widgets")) {
                assertNull(w.getAsJsonObject().get("writeVerb"));
            }
        }
    }

    private static List<String> strings(JsonArray arr) {
        List<String> out = new ArrayList<>();
        for (JsonElement e : arr) {
            out.add(e.getAsString());
        }
        return out;
    }

    private static List<String> verbsOf(JsonObject panel) {
        return strings(panel.getAsJsonArray("verbs"));
    }
}
