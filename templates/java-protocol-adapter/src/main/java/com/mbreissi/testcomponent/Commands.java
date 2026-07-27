package <<PACKAGE>>;

import com.mbreissi.edgecommons.commands.CommandException;
import com.mbreissi.edgecommons.commands.CommandInbox;
import com.mbreissi.edgecommons.messaging.Message;
import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonNull;
import com.google.gson.JsonObject;

import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * The southbound command surface — the {@code sb/*} verbs + the three edge-console panels.
 *
 * <p>This class owns the whole {@code gg.getCommands()} registration for {@code <<COMPONENTNAME>>}:
 * {@code sb/status}, {@code sb/read}, {@code sb/write}, {@code sb/signals}, {@code sb/browse},
 * {@code sb/pause}, {@code sb/resume}, {@code reconnect}, {@code repoll}. It is the generic southbound
 * command family (SOUTHBOUND.md §2.2) every adapter serves — a real adapter changes the <i>seam</i>
 * behind it ({@link DeviceControl}), not this surface.
 *
 * <h2>Conventions every verb follows</h2>
 * <ul>
 *   <li><b>Instance routing (D-EIP-13):</b> {@code body.instance} is optional iff exactly one device
 *       is configured; with two or more, a missing id is {@code BAD_ARGS} and an unknown id is
 *       {@code NO_SUCH_INSTANCE}.</li>
 *   <li><b>Standardized error codes:</b> {@code BAD_ARGS}, {@code NO_SUCH_INSTANCE},
 *       {@code WRITE_NOT_ALLOWED}, {@code WRITE_FAILED}, {@code DEVICE_UNAVAILABLE},
 *       {@code READ_FAILED}, {@code RECONNECT_FAILED}, {@code BROWSE_UNSUPPORTED},
 *       {@code BROWSE_FAILED}, {@code PAUSED}.</li>
 *   <li><b>The session is never touched here.</b> Every verb that reads/writes/reconnects/pauses is
 *       routed through the device's own {@link DeviceControl} — the worker serializes it with the poll
 *       loop and <i>confirms</i> it — so the command surface never races the poll loop on the same
 *       connection.</li>
 *   <li><b>{@code sb/write} allow-lists BEFORE any device I/O.</b> A refused entry never reaches
 *       {@link DeviceControl#write} — an adapter that writes whatever it is asked to is a
 *       control-system vulnerability, not a feature.</li>
 *   <li>Every verb records into the {@code <<COMPONENTNAME>>Command} metric family
 *       ({@code instance}×{@code verb}×{@code result}).</li>
 * </ul>
 *
 * <p>Three panels ({@code overview}, {@code signals}, {@code diagnostics}) are registered via
 * {@link CommandInbox#registerPanel} for the edge-console descriptor surface — each
 * {@code scope: "instance"}, {@code order} 10/20/30.
 */
public final class Commands {

    private Commands() {
    }

    /**
     * Register every {@code sb/*} verb + the three edge-console panels on the inbox.
     *
     * @param commands the command inbox (from {@code gg.getCommands()})
     * @param handles  the per-device handles the command surface routes on
     */
    public static void registerAll(CommandInbox commands, List<DeviceHandle> handles) {
        Commander commander = new Commander(handles);

        commands.register("sb/status", req -> commander.status(bodyOf(req)));
        commands.register("sb/read", req -> commander.read(bodyOf(req)));
        commands.register("sb/write", req -> commander.write(bodyOf(req)));
        commands.register("sb/signals", req -> commander.signals(bodyOf(req)));
        commands.register("sb/browse", req -> commander.browse(bodyOf(req)));
        commands.register("sb/pause", req -> commander.pause(bodyOf(req)));
        commands.register("sb/resume", req -> commander.resume(bodyOf(req)));
        commands.register("reconnect", req -> commander.reconnect(bodyOf(req)));
        commands.register("repoll", req -> commander.repoll(bodyOf(req)));

        for (JsonObject panel : panels()) {
            commands.registerPanel(panel);
        }
    }

    /** The request body as a {@link JsonObject} (an empty object when the payload is not one). */
    private static JsonObject bodyOf(Message request) {
        Object body = request.getBody();
        return body instanceof JsonObject jo ? jo : new JsonObject();
    }

    /**
     * The three edge-console panel descriptors — renderable by the shipped descriptor renderer:
     * {@code summary.rows}, {@code commandSummary.verbs}, a {@code signalGrid} bound to
     * {@code sb/signals}/{@code sb/read}, and a hierarchical {@code treeBrowser} bound to
     * {@code sb/browse}/{@code sb/read}. Core validates {@code id}/{@code title}/uniqueness; the widget
     * kinds and bound verbs are console-interpreted, so they ride verbatim. {@code order} 10/20/30,
     * {@code scope: "instance"} on each view and on every command-backed widget.
     */
    public static List<JsonObject> panels() {
        List<JsonObject> out = new ArrayList<>();

        JsonObject overview = panel("overview", "Overview", 10);
        JsonArray overviewWidgets = new JsonArray();
        overviewWidgets.add(summaryWidget("overview-summary", "Adapter overview",
                row("Signals", "Configured signal inventory via cmd/sb/signals"),
                row("Lifecycle", "Pause, resume, reconnect, and repoll the instance"),
                row("Writes", "Allow-listed via writes.allow[]; checked before device I/O")));
        overviewWidgets.add(commandSummaryWidget("overview-lifecycle", "Lifecycle bindings",
                "sb/status", "reconnect", "sb/pause", "sb/resume", "repoll"));
        overview.add("widgets", overviewWidgets);
        overview.add("verbs", arr("sb/status", "reconnect", "sb/pause", "sb/resume"));
        out.add(overview);

        JsonObject signals = panel("signals", "Signals", 20);
        JsonArray signalsWidgets = new JsonArray();
        signalsWidgets.add(signalGridWidget("configured-signals", "Configured signals",
                "sb/signals", "sb/read"));
        signals.add("widgets", signalsWidgets);
        signals.add("verbs", arr("sb/signals", "sb/read", "sb/write", "repoll"));
        out.add(signals);

        JsonObject diagnostics = panel("diagnostics", "Diagnostics", 30);
        JsonArray diagWidgets = new JsonArray();
        diagWidgets.add(treeBrowserWidget("inventory-tree", "Inventory", "sb/browse", "sb/read"));
        diagWidgets.add(commandSummaryWidget("diagnostic-commands", "Diagnostic commands",
                "sb/status", "sb/browse"));
        diagnostics.add("widgets", diagWidgets);
        diagnostics.add("verbs", arr("sb/browse", "sb/status"));
        out.add(diagnostics);

        return out;
    }

    private static JsonObject panel(String id, String title, int order) {
        JsonObject p = new JsonObject();
        p.addProperty("id", id);
        p.addProperty("title", title);
        p.addProperty("order", order);
        p.addProperty("scope", "instance");
        return p;
    }

    private static JsonObject baseWidget(String kind, String id, String title) {
        JsonObject w = new JsonObject();
        w.addProperty("kind", kind);
        w.addProperty("id", id);
        w.addProperty("title", title);
        return w;
    }

    private static JsonObject summaryWidget(String id, String title, JsonObject... rows) {
        JsonObject w = baseWidget("summary", id, title);
        JsonArray rowArray = new JsonArray();
        for (JsonObject row : rows) {
            rowArray.add(row);
        }
        w.add("rows", rowArray);
        return w;
    }

    private static JsonObject commandSummaryWidget(String id, String title, String... verbs) {
        JsonObject w = baseWidget("commandSummary", id, title);
        w.add("verbs", arr(verbs));
        return w;
    }

    private static JsonObject treeBrowserWidget(String id, String title,
                                                String browseVerb, String readVerb) {
        JsonObject w = baseWidget("treeBrowser", id, title);
        w.addProperty("scope", "instance");
        w.addProperty("mode", "hierarchical");
        w.addProperty("rootRef", "root");
        w.addProperty("depth", 1);
        w.addProperty("maxRefs", 200);
        w.addProperty("browseVerb", browseVerb);
        w.addProperty("readVerb", readVerb);
        return w;
    }

    private static JsonObject signalGridWidget(String id, String title,
                                               String signalsVerb, String readVerb) {
        JsonObject w = baseWidget("signalGrid", id, title);
        w.addProperty("scope", "instance");
        w.addProperty("signalsVerb", signalsVerb);
        // Descriptor-compat hint: the shipped edge-console signalGrid reads `subscriptionsVerb`
        // (falling back to the removed `sb/subscriptions`). Point that key at the `sb/signals`
        // verb too, so the current console binds correctly until it reads `signalsVerb`. This is a
        // descriptor field alias, NOT a wire-verb alias — no `sb/subscriptions` verb exists here.
        w.addProperty("subscriptionsVerb", signalsVerb);
        w.addProperty("readVerb", readVerb);
        return w;
    }

    private static JsonObject row(String label, String value) {
        JsonObject row = new JsonObject();
        row.addProperty("label", label);
        row.addProperty("value", value);
        return row;
    }

    private static JsonArray arr(String... values) {
        JsonArray a = new JsonArray();
        for (String v : values) {
            a.add(v);
        }
        return a;
    }

    // =============================================================================================
    // The per-device command dispatcher
    // =============================================================================================

    /**
     * The per-device handle the command surface routes on: the config (routing, allow-list, inventory),
     * the control seam (session-touching verbs), the shared health (status/paused), and the metrics
     * emitter (per-verb command counters).
     */
    static final class DeviceHandle {
        final DeviceConfig cfg;
        final DeviceControl control;
        final Health health;
        final DeviceMetrics dm;
        /** The signal inventory {@code sb/signals} returns — a config/backend view, no device round-trip. */
        final List<Device.SignalInfo> signals;

        DeviceHandle(DeviceConfig cfg, DeviceControl control, Health health, DeviceMetrics dm,
                     List<Device.SignalInfo> signals) {
            this.cfg = cfg;
            this.control = control;
            this.health = health;
            this.dm = dm;
            this.signals = signals;
        }
    }

    /**
     * The seam the command surface routes session-touching verbs through. The real implementation (the
     * device worker in the app) serializes each call with the poll loop and confirms it; the tests
     * supply a mock. The command layer never touches the protocol session directly.
     */
    interface DeviceControl {

        /** Live-read these ids now ({@code sb/read}). Serializes with the loop and works while paused. */
        List<Device.Reading> readNow(List<String> ids)
                throws ReadFailedException, DeviceUnavailableException;

        /**
         * A confirmed, allow-listed write ({@code sb/write}). The allow-list is checked in the command
         * layer BEFORE this is ever called.
         */
        void write(String signalId, JsonElement value)
                throws WriteFailedException, DeviceUnavailableException;

        /** One page of address-space discovery ({@code sb/browse}). */
        Device.BrowsePage browse(String cursor, int max)
                throws Device.BrowseException, DeviceUnavailableException;

        /** Pause telemetry production ({@code sb/pause}); returns whether the state changed. */
        boolean pause();

        /** Resume telemetry production ({@code sb/resume}); returns whether the state changed. */
        boolean resume();

        /** Drop + re-establish, one immediate attempt ({@code reconnect}). */
        void reconnect() throws ReconnectFailedException, DeviceUnavailableException;

        /** Force an immediate poll now ({@code repoll}); returns signals read. */
        long repoll() throws DeviceUnavailableException;
    }

    /** The device task is unavailable (worker gone, or the session is disconnected). */
    static final class DeviceUnavailableException extends Exception {
        DeviceUnavailableException(String message) {
            super(message);
        }

        static DeviceUnavailableException gone() {
            return new DeviceUnavailableException("device task is unavailable");
        }
    }

    /** A read reached the device and the link failed ({@code READ_FAILED}). */
    static final class ReadFailedException extends Exception {
        ReadFailedException(String message) {
            super(message);
        }
    }

    /** The device rejected a write ({@code WRITE_FAILED} when every attempted write failed). */
    static final class WriteFailedException extends Exception {
        WriteFailedException(String message) {
            super(message);
        }
    }

    /** An immediate reconnect attempt failed ({@code RECONNECT_FAILED}). */
    static final class ReconnectFailedException extends Exception {
        ReconnectFailedException(String message) {
            super(message);
        }
    }

    /**
     * The command dispatcher: owns the per-device handles + the config order (for the single-instance
     * default). Each verb returns the reply's {@code result} object, or throws {@link CommandException}
     * with a standardized code.
     */
    static final class Commander {

        private final Map<String, DeviceHandle> devices = new LinkedHashMap<>();
        private final List<String> ids = new ArrayList<>();

        Commander(List<DeviceHandle> handles) {
            for (DeviceHandle h : handles) {
                ids.add(h.cfg.id());
                devices.put(h.cfg.id(), h);
            }
        }

        /**
         * Route to the addressed device (D-EIP-13): {@code body.instance} optional iff exactly one
         * device is configured; with two or more a missing/unknown id is {@code BAD_ARGS} /
         * {@code NO_SUCH_INSTANCE}.
         */
        DeviceHandle resolve(JsonObject body) throws CommandException {
            String id = str(body, "instance");
            if (id != null) {
                DeviceHandle h = devices.get(id);
                if (h == null) {
                    throw new CommandException("NO_SUCH_INSTANCE", "no configured device `" + id + "`");
                }
                return h;
            }
            if (ids.size() == 1) {
                return devices.get(ids.get(0));
            }
            throw new CommandException("BAD_ARGS",
                    "field `instance` is required when multiple devices are configured");
        }

        // --- sb/status -----------------------------------------------------------------------------

        JsonObject status(JsonObject body) throws CommandException {
            DeviceHandle h = resolve(body);
            long started = System.nanoTime();
            LinkState link = h.health.link();
            boolean connected = link == LinkState.ONLINE;
            boolean paused = h.health.isPaused();
            String state = paused && connected ? "PAUSED" : link.asString();
            JsonObject out = new JsonObject();
            out.addProperty("id", h.cfg.id());
            out.addProperty("adapter", h.cfg.adapter());
            out.addProperty("connected", connected);
            out.addProperty("state", state);
            out.addProperty("paused", paused);
            out.addProperty("endpoint", h.cfg.connection().endpoint());
            out.add("metrics", h.dm.countersView());
            h.dm.recordCommand("sb/status", true, ms(started));
            return out;
        }

        // --- sb/signals (the configured inventory, no device I/O) ----------------------------------

        JsonObject signals(JsonObject body) throws CommandException {
            DeviceHandle h = resolve(body);
            long started = System.nanoTime();
            JsonArray signals = new JsonArray();
            for (Device.SignalInfo s : h.signals) {
                JsonObject o = new JsonObject();
                o.addProperty("id", s.id());
                o.addProperty("name", s.name());
                o.addProperty("writable", h.cfg.writes().permits(s.id()));
                signals.add(o);
            }
            h.dm.recordCommand("sb/signals", true, ms(started));
            JsonObject out = new JsonObject();
            out.addProperty("id", h.cfg.id());
            out.add("signals", signals);
            return out;
        }

        // --- sb/read (on-demand read of named signals) ---------------------------------------------

        JsonObject read(JsonObject body) throws CommandException {
            DeviceHandle h = resolve(body);
            long started = System.nanoTime();
            JsonArray refs = arrOrNull(body, "signals");
            if (refs == null) {
                throw new CommandException("BAD_ARGS", "expected a `signals` array");
            }

            // Resolve each ref to a stable id (keeping the request order for the reply).
            List<Ref> plan = new ArrayList<>();
            List<String> requestIds = new ArrayList<>();
            for (JsonElement e : refs) {
                Ref ref = resolveRef(h, e.isJsonObject() ? e.getAsJsonObject() : new JsonObject());
                plan.add(ref);
                if (ref.id != null) {
                    requestIds.add(ref.id);
                }
            }

            Map<String, Device.Reading> readings = new LinkedHashMap<>();
            if (!requestIds.isEmpty()) {
                try {
                    for (Device.Reading r : h.control.readNow(requestIds)) {
                        readings.put(r.signalId(), r);
                    }
                } catch (ReadFailedException e) {
                    h.dm.recordCommand("sb/read", false, ms(started));
                    throw new CommandException("READ_FAILED", e.getMessage());
                } catch (DeviceUnavailableException e) {
                    h.dm.recordCommand("sb/read", false, ms(started));
                    throw new CommandException("DEVICE_UNAVAILABLE", e.getMessage());
                }
            }

            JsonArray reads = new JsonArray();
            for (Ref ref : plan) {
                if (ref.id != null) {
                    Device.Reading r = readings.get(ref.id);
                    if (r != null) {
                        JsonObject o = new JsonObject();
                        JsonObject sig = new JsonObject();
                        sig.addProperty("id", ref.id);
                        o.add("signal", sig);
                        o.add("value", r.value() != null ? r.value() : JsonNull.INSTANCE);
                        o.addProperty("quality", r.quality().name());
                        o.addProperty("qualityRaw", r.qualityRaw());
                        reads.add(o);
                    } else {
                        reads.add(badRead(ref.id, "NO_DATA"));
                    }
                } else {
                    reads.add(badRead(ref.label, "UNRESOLVED_REF"));
                }
            }

            h.dm.recordCommand("sb/read", true, ms(started));
            JsonObject out = new JsonObject();
            out.addProperty("id", h.cfg.id());
            out.add("reads", reads);
            return out;
        }

        // --- sb/write (§2.2 batch shape; allow-list BEFORE any device I/O; confirmed) --------------

        JsonObject write(JsonObject body) throws CommandException {
            DeviceHandle h = resolve(body);
            long started = System.nanoTime();
            List<JsonObject> entries = writeEntries(body);

            JsonArray results = new JsonArray();
            int refused = 0;
            int attempted = 0;
            int succeeded = 0;

            for (JsonObject entry : entries) {
                Ref ref = resolveRef(h, entry);
                if (ref.id == null) {
                    results.add(writeResult(ref.label, null, false, "unresolved ref"));
                    continue;
                }
                // THE ALLOW-LIST — checked here, BEFORE the write ever reaches the device.
                if (!h.cfg.writes().permits(ref.id)) {
                    refused++;
                    results.add(writeResult(ref.id, null, false, "not in writes.allow"));
                    continue;
                }
                JsonElement value = entry.has("value") ? entry.get("value") : null;
                if (value == null) {
                    results.add(writeResult(ref.id, null, false, "missing value"));
                    continue;
                }

                attempted++;
                // `writeErrors` mirrors `readErrors`: DEVICE-path failures only. An entry counts iff
                // it passed validation + the allow-list and then failed at the device; caller/policy
                // errors (unresolved refs, allow-list refusals, missing values) do not.
                try {
                    h.control.write(ref.id, value);
                    succeeded++;
                    results.add(writeResult(ref.id, value, true, null));
                } catch (WriteFailedException e) {
                    h.health.incrementWriteErrors();
                    results.add(writeResult(ref.id, value, false, e.getMessage()));
                } catch (DeviceUnavailableException e) {
                    h.health.incrementWriteErrors();
                    h.dm.recordCommand("sb/write", false, ms(started));
                    throw new CommandException("DEVICE_UNAVAILABLE", e.getMessage());
                }
            }

            // WRITE_NOT_ALLOWED only when EVERY entry was an allow-list refusal (nothing else attempted).
            if (!entries.isEmpty() && refused == entries.size()) {
                h.dm.recordCommand("sb/write", false, ms(started));
                throw new CommandException("WRITE_NOT_ALLOWED",
                        "no entry is in this instance's writes.allow list");
            }
            // WRITE_FAILED when every allowed write reached the device and every one failed.
            if (attempted > 0 && succeeded == 0) {
                h.dm.recordCommand("sb/write", false, ms(started));
                throw new CommandException("WRITE_FAILED",
                        "every attempted write was rejected by the device");
            }

            h.dm.recordCommand("sb/write", true, ms(started));
            JsonObject out = new JsonObject();
            out.addProperty("id", h.cfg.id());
            out.addProperty("written", succeeded);
            out.add("results", results);
            return out;
        }

        // --- sb/browse (paged address-space discovery + the hierarchical panel mode) ---------------

        JsonObject browse(JsonObject body) throws CommandException {
            DeviceHandle h = resolve(body);
            long started = System.nanoTime();
            // Presence of `ref` (or its companions `depth`/`maxRefs`) selects the hierarchical panel
            // mode; mixing the two request families is BAD_ARGS.
            boolean hierArgs = body.has("ref") || body.has("depth") || body.has("maxRefs");
            boolean pagedArgs = body.has("cursor") || body.has("max");
            if (hierArgs && pagedArgs) {
                h.dm.recordCommand("sb/browse", false, ms(started));
                throw new CommandException("BAD_ARGS",
                        "`ref`/`depth`/`maxRefs` (hierarchical) and `cursor`/`max` (paged) are mutually exclusive");
            }
            if (hierArgs) {
                return browseHierarchical(h, body, started);
            }
            String cursor = str(body, "cursor");
            int max = 200;
            if (body.has("max") && body.get("max").isJsonPrimitive()) {
                max = Math.max(1, Math.min(1000, body.get("max").getAsInt()));
            }

            try {
                Device.BrowsePage page = h.control.browse(cursor, max);
                JsonArray entries = new JsonArray();
                for (Device.BrowsedSignal e : page.entries()) {
                    JsonObject o = new JsonObject();
                    o.addProperty("id", e.id());
                    o.addProperty("name", e.name());
                    o.addProperty("type", e.typeName());
                    entries.add(o);
                }
                JsonObject out = new JsonObject();
                out.addProperty("id", h.cfg.id());
                out.add("entries", entries);
                if (page.nextCursor() != null) {
                    out.addProperty("cursor", page.nextCursor());
                }
                h.dm.recordCommand("sb/browse", true, ms(started));
                return out;
            } catch (Device.BrowseException e) {
                h.dm.recordCommand("sb/browse", false, ms(started));
                if (e.isUnsupported()) {
                    throw new CommandException("BROWSE_UNSUPPORTED", e.getMessage());
                }
                throw new CommandException("BROWSE_FAILED", e.getMessage());
            } catch (DeviceUnavailableException e) {
                h.dm.recordCommand("sb/browse", false, ms(started));
                throw new CommandException("DEVICE_UNAVAILABLE", e.getMessage());
            }
        }

        /**
         * The hierarchical {@code sb/browse} mode the edge-console {@code treeBrowser} sends
         * ({@code {instance, ref, depth?, maxRefs?}}). Serves the SAME {@link Device.BrowsedSignal}
         * inventory as the paged mode, shaped as a node tree: {@code ref: "root"} answers the device
         * node whose {@code contains} refs are the browsed signals (bounded by {@code maxRefs});
         * a signal id as {@code ref} answers that node with {@code "refs": []} (a known leaf); an
         * unknown ref is {@code BAD_ARGS}. {@code depth} is bounded 1..4 and honored, but the
         * template's tree is flat — depth beyond 1 finds no grandchildren.
         */
        private JsonObject browseHierarchical(DeviceHandle h, JsonObject body, long started)
                throws CommandException {
            String ref = str(body, "ref");
            if (ref == null) {
                h.dm.recordCommand("sb/browse", false, ms(started));
                throw new CommandException("BAD_ARGS", "hierarchical browse requires a `ref` string");
            }
            int depth = boundedInt(body, "depth", 1, 1, 4);
            int maxRefs = boundedInt(body, "maxRefs", 200, 1, 1000);

            // Drain the same paged inventory the plain mode serves. For `root`, one entry past
            // maxRefs is enough to know the reply is truncated; a leaf lookup drains every page.
            List<Device.BrowsedSignal> inventory = new ArrayList<>();
            int limit = "root".equals(ref) ? maxRefs + 1 : Integer.MAX_VALUE;
            try {
                String cursor = null;
                do {
                    Device.BrowsePage page = h.control.browse(cursor, 1000);
                    inventory.addAll(page.entries());
                    cursor = page.nextCursor();
                } while (cursor != null && inventory.size() < limit);
            } catch (Device.BrowseException e) {
                h.dm.recordCommand("sb/browse", false, ms(started));
                if (e.isUnsupported()) {
                    throw new CommandException("BROWSE_UNSUPPORTED", e.getMessage());
                }
                throw new CommandException("BROWSE_FAILED", e.getMessage());
            } catch (DeviceUnavailableException e) {
                h.dm.recordCommand("sb/browse", false, ms(started));
                throw new CommandException("DEVICE_UNAVAILABLE", e.getMessage());
            }

            JsonObject root;
            int refCount;
            boolean truncated;
            if ("root".equals(ref)) {
                JsonArray refs = new JsonArray();
                int n = Math.min(inventory.size(), maxRefs);
                for (int i = 0; i < n; i++) {
                    Device.BrowsedSignal e = inventory.get(i);
                    JsonObject contains = new JsonObject();
                    contains.addProperty("referenceType", "contains");
                    contains.add("target", node(e.id(), e.name(), "signal", e.typeName()));
                    refs.add(contains);
                }
                root = node("root", h.cfg.id(), "device", null);
                root.add("refs", refs);
                refCount = n;
                truncated = inventory.size() > maxRefs;
            } else {
                Device.BrowsedSignal found = null;
                for (Device.BrowsedSignal e : inventory) {
                    if (e.id().equals(ref)) {
                        found = e;
                        break;
                    }
                }
                if (found == null) {
                    h.dm.recordCommand("sb/browse", false, ms(started));
                    throw new CommandException("BAD_ARGS", "unknown ref `" + ref + "`");
                }
                root = node(found.id(), found.name(), "signal", found.typeName());
                root.add("refs", new JsonArray()); // a known leaf
                refCount = 0;
                truncated = false;
            }

            JsonObject out = new JsonObject();
            out.addProperty("id", h.cfg.id());
            out.addProperty("mode", "hierarchical");
            out.add("root", root);
            out.addProperty("refCount", refCount);
            out.addProperty("depth", depth);
            out.addProperty("truncated", truncated);
            h.dm.recordCommand("sb/browse", true, ms(started));
            return out;
        }

        // --- sb/pause + sb/resume (idempotent {paused, changed}) -----------------------------------

        JsonObject pause(JsonObject body) throws CommandException {
            DeviceHandle h = resolve(body);
            long started = System.nanoTime();
            boolean changed = h.control.pause();
            h.dm.recordCommand("sb/pause", true, ms(started));
            JsonObject out = new JsonObject();
            out.addProperty("id", h.cfg.id());
            out.addProperty("paused", true);
            out.addProperty("changed", changed);
            return out;
        }

        JsonObject resume(JsonObject body) throws CommandException {
            DeviceHandle h = resolve(body);
            long started = System.nanoTime();
            boolean changed = h.control.resume();
            h.dm.recordCommand("sb/resume", true, ms(started));
            JsonObject out = new JsonObject();
            out.addProperty("id", h.cfg.id());
            out.addProperty("paused", false);
            out.addProperty("changed", changed);
            return out;
        }

        // --- reconnect -----------------------------------------------------------------------------

        JsonObject reconnect(JsonObject body) throws CommandException {
            DeviceHandle h = resolve(body);
            long started = System.nanoTime();
            try {
                h.control.reconnect();
                h.dm.recordCommand("reconnect", true, ms(started));
                JsonObject out = new JsonObject();
                out.addProperty("id", h.cfg.id());
                out.addProperty("connected", true);
                return out;
            } catch (ReconnectFailedException e) {
                h.dm.recordCommand("reconnect", false, ms(started));
                throw new CommandException("RECONNECT_FAILED", e.getMessage());
            } catch (DeviceUnavailableException e) {
                h.dm.recordCommand("reconnect", false, ms(started));
                throw new CommandException("DEVICE_UNAVAILABLE", e.getMessage());
            }
        }

        // --- repoll (refused with PAUSED while paused) ---------------------------------------------

        JsonObject repoll(JsonObject body) throws CommandException {
            DeviceHandle h = resolve(body);
            long started = System.nanoTime();
            if (h.health.isPaused()) {
                h.dm.recordCommand("repoll", false, ms(started));
                throw new CommandException("PAUSED", "instance is paused - resume first");
            }
            try {
                long polled = h.control.repoll();
                h.dm.recordCommand("repoll", true, ms(started));
                JsonObject out = new JsonObject();
                out.addProperty("id", h.cfg.id());
                out.addProperty("polled", polled);
                return out;
            } catch (DeviceUnavailableException e) {
                h.dm.recordCommand("repoll", false, ms(started));
                throw new CommandException("DEVICE_UNAVAILABLE", e.getMessage());
            }
        }

        /**
         * Resolve a {@code sb/read}/{@code sb/write} signal-ref to its stable id: {@code {"signalId"}} /
         * {@code {"id"}} directly, or {@code {"name"}} looked up against the configured inventory. When
         * unresolved, {@link Ref#id} is null and {@link Ref#label} carries the offending label.
         */
        private Ref resolveRef(DeviceHandle h, JsonObject r) {
            String signalId = str(r, "signalId");
            if (signalId != null) {
                return Ref.resolved(signalId);
            }
            String id = str(r, "id");
            if (id != null) {
                return Ref.resolved(id);
            }
            String name = str(r, "name");
            if (name != null) {
                for (Device.SignalInfo s : h.signals) {
                    if (name.equals(s.name())) {
                        return Ref.resolved(s.id());
                    }
                }
                return Ref.unresolved(name);
            }
            return Ref.unresolved("<invalid ref>");
        }
    }

    /** A resolved signal id, or an unresolved label. */
    private record Ref(String id, String label) {
        static Ref resolved(String id) {
            return new Ref(id, null);
        }

        static Ref unresolved(String label) {
            return new Ref(null, label);
        }
    }

    // =============================================================================================
    // Helpers
    // =============================================================================================

    private static long ms(long startedNanos) {
        return Math.max(0L, (System.nanoTime() - startedNanos) / 1_000_000L);
    }

    private static String str(JsonObject o, String key) {
        if (o.has(key) && o.get(key).isJsonPrimitive() && o.getAsJsonPrimitive(key).isString()) {
            return o.get(key).getAsString();
        }
        return null;
    }

    private static JsonArray arrOrNull(JsonObject o, String key) {
        return o.has(key) && o.get(key).isJsonArray() ? o.getAsJsonArray(key) : null;
    }

    /** An integer body field, defaulted when absent/malformed and clamped into {@code [lo, hi]}. */
    private static int boundedInt(JsonObject o, String key, int def, int lo, int hi) {
        int v = def;
        if (o.has(key) && o.get(key).isJsonPrimitive()) {
            try {
                v = o.get(key).getAsInt();
            } catch (RuntimeException e) {
                v = def;
            }
        }
        return Math.max(lo, Math.min(hi, v));
    }

    /** One hierarchical-browse node ({@code nodeId, name, nodeClass, dataType}); refs are added by the caller. */
    private static JsonObject node(String nodeId, String name, String nodeClass, String dataType) {
        JsonObject o = new JsonObject();
        o.addProperty("nodeId", nodeId);
        o.addProperty("name", name);
        o.addProperty("nodeClass", nodeClass);
        if (dataType == null) {
            o.add("dataType", JsonNull.INSTANCE);
        } else {
            o.addProperty("dataType", dataType);
        }
        return o;
    }

    private static JsonObject badRead(String id, String raw) {
        JsonObject o = new JsonObject();
        JsonObject sig = new JsonObject();
        sig.addProperty("id", id);
        o.add("signal", sig);
        o.add("value", JsonNull.INSTANCE);
        o.addProperty("quality", "BAD");
        o.addProperty("qualityRaw", raw);
        return o;
    }

    private static JsonObject writeResult(String signal, JsonElement value, boolean ok, String error) {
        JsonObject o = new JsonObject();
        o.addProperty("signal", signal);
        if (value != null) {
            o.add("value", value);
        }
        o.addProperty("ok", ok);
        if (error != null) {
            o.addProperty("error", error);
        }
        return o;
    }

    /**
     * Normalize an {@code sb/write} body to a list of {@code {ref…, value}} entries: a {@code writes}
     * array, or a single object carrying {@code value} (§2.2). Throws {@code BAD_ARGS} when neither
     * form is present.
     */
    private static List<JsonObject> writeEntries(JsonObject body) throws CommandException {
        List<JsonObject> out = new ArrayList<>();
        if (body.has("writes") && body.get("writes").isJsonArray()) {
            for (JsonElement e : body.getAsJsonArray("writes")) {
                out.add(e.isJsonObject() ? e.getAsJsonObject() : new JsonObject());
            }
            return out;
        }
        if (body.has("value")) {
            out.add(body);
            return out;
        }
        throw new CommandException("BAD_ARGS",
                "expected a `writes` array or a single write object with `value`");
    }
}
