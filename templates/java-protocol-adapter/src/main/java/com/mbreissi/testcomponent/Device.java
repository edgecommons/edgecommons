package <<PACKAGE>>;

import com.google.gson.JsonElement;
import com.google.gson.JsonNull;
import com.google.gson.JsonObject;
import com.google.gson.JsonPrimitive;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.ArrayList;
import java.util.List;

/**
 * The device seam: what a <i>protocol adapter</i> talks to.
 *
 * <p>{@link DeviceSession} is one live connection to one device. Implement it once per protocol —
 * Modbus, OPC UA, whatever you are bridging — and everything above it (the connection lifecycle,
 * backoff, publishing, health) is written against the interface and never learns your protocol.
 *
 * <p><b>The boundary rule, and it is worth enforcing in review:</b> a backend knows protocols. It
 * does <b>not</b> know EdgeCommons topics, the UNS, message envelopes, or metrics — which is why this
 * file imports none of {@code com.mbreissi.edgecommons}. If your {@code DeviceSession} reaches for the
 * UNS or a facade, the seam has leaked. (That is also why the seam carries its own {@link Quality}
 * enum rather than the library's {@code facades.Quality}: the mapping happens one layer up, in the
 * app, not in the backend.)
 *
 * <h2>Signals, not tags</h2>
 * A <b>signal</b> is one data point — a measured value with identity, quality, and timestamps.
 * (OPC UA calls it a "tag"; Modbus calls it a "register".) The word "tag" is reserved in EdgeCommons
 * for the envelope's <i>business metadata</i>, which is a different thing entirely.
 *
 * <h2>Quality is not optional</h2>
 * Every sample carries a quality normalized to {@code GOOD | BAD | UNCERTAIN}, plus the native code
 * in {@code qualityRaw} for diagnosis. This is what lets a consumer gate on quality without knowing
 * your protocol — and it is why a read failure must be published as a {@code BAD} sample rather than
 * swallowed. A signal that silently stops updating is indistinguishable from one that is simply not
 * changing.
 *
 * <p>The types here (interfaces, records, the simulated backend) live under this one namespace class
 * so a real adapter can find the whole seam in a single file; replace {@link SimBackend} with your
 * protocol.
 */
public final class Device {

    private Device() {
    }

    /** Resolve a configured {@code adapter} name to its backend. A real adapter matches its protocols here. */
    static DeviceBackend backendFor(String adapter) {
        if ("sim".equals(adapter)) {
            return new SimBackend();
        }
        return null;
    }

    /**
     * Normalized quality. The protocol's own status code goes in {@code qualityRaw}.
     *
     * <p>{@code UNCERTAIN} is unused by the simulated backend and used constantly by real ones: a
     * stale cached read, a value outside its calibrated range, a sensor that answered but warned.
     */
    enum Quality {
        GOOD,
        BAD,
        UNCERTAIN
    }

    /** One reading from the device. */
    record Reading(String signalId, String name, JsonElement value, Quality quality, String qualityRaw) {
    }

    /**
     * One signal in the adapter's inventory — its stable id and human label, known from config/backend
     * <b>without a device round-trip</b>. Backs the {@code sb/signals} command.
     */
    record SignalInfo(String id, String name) {
    }

    /**
     * One entry discovered by {@link DeviceSession#browse} — a signal the device <i>offers</i>, whether
     * or not it is configured. Backs the {@code sb/browse} diagnostics surface.
     */
    record BrowsedSignal(String id, String name, String typeName) {
    }

    /**
     * One page of a {@link DeviceSession#browse} enumeration. Browsing is <b>paged</b> because a
     * device's address space can be large; {@code nextCursor} is non-null while more pages remain.
     */
    record BrowsePage(List<BrowsedSignal> entries, String nextCursor) {
        static BrowsePage empty() {
            return new BrowsePage(List.of(), null);
        }
    }

    /**
     * How to reach one device. Deliberately open ({@code additionalProperties} in the schema): every
     * protocol needs different keys, and this is the one place the adapter should not be strict.
     */
    record ConnectionConfig(String endpoint, JsonObject extra) {
        static ConnectionConfig from(JsonObject connection) {
            if (connection == null) {
                return new ConnectionConfig("", new JsonObject());
            }
            String endpoint = connection.has("endpoint") && connection.get("endpoint").isJsonPrimitive()
                    ? connection.get("endpoint").getAsString() : "";
            return new ConnectionConfig(endpoint, connection);
        }
    }

    /**
     * Why talking to the device failed — and whether reconnecting could help. A {@code transient}
     * failure (the link is down, the device is busy) is worth retrying; a {@code permanent} one (a bad
     * endpoint, a rejected credential) fails identically forever, so the supervisor backs off hard
     * rather than hammering.
     */
    static final class DeviceException extends Exception {
        private final boolean transientError;

        private DeviceException(String message, boolean transientError) {
            super(message);
            this.transientError = transientError;
        }

        static DeviceException transientError(String message) {
            return new DeviceException(message, true);
        }

        static DeviceException permanent(String message) {
            return new DeviceException(message, false);
        }

        boolean isTransient() {
            return transientError;
        }
    }

    /**
     * Why a {@code sb/browse} could not answer. Kept distinct from {@link DeviceException} because
     * "this protocol has no discovery" is a permanent, honest capability limit — not a link failure.
     */
    static final class BrowseException extends Exception {
        private final boolean unsupported;

        private BrowseException(String message, boolean unsupported) {
            super(message);
            this.unsupported = unsupported;
        }

        /** The protocol has no discovery service. Maps to {@code BROWSE_UNSUPPORTED}. */
        static BrowseException unsupported() {
            return new BrowseException("this adapter has no discovery service", true);
        }

        /** A mid-browse failure (a link error, a malformed reply). Maps to {@code BROWSE_FAILED}. */
        static BrowseException failed(String message) {
            return new BrowseException(message, false);
        }

        boolean isUnsupported() {
            return unsupported;
        }
    }

    /** A live connection to one device. <b>This is the interface you implement.</b> */
    interface DeviceSession {

        /**
         * Read the configured signals once.
         *
         * <p>A read that fails for <i>one</i> signal should return that signal with {@link Quality#BAD}
         * rather than failing the whole call — one dead register must not blind you to the other
         * ninety-nine. Throw only when the <i>connection</i> is broken.
         */
        List<Reading> readSignals() throws DeviceException;

        /**
         * Read a named subset <b>now</b> (backs {@code sb/read}). The default reads everything and
         * filters, which is correct for any backend; override it when your protocol can read a subset
         * more cheaply. Throws only when the connection is broken.
         */
        default List<Reading> readNamed(List<String> ids) throws DeviceException {
            List<Reading> out = new ArrayList<>();
            for (Reading r : readSignals()) {
                if (ids.contains(r.signalId())) {
                    out.add(r);
                }
            }
            return out;
        }

        /** Write a value back to the device. Throws if the write is rejected, or the link is down. */
        void writeSignal(String signalId, JsonElement value) throws DeviceException;

        /**
         * Enumerate the device's address space, one page at a time (backs {@code sb/browse}).
         *
         * <p>The default throws {@link BrowseException#unsupported()} — a protocol with no discovery
         * (Modbus, a fixed register map) is honest to leave it unimplemented. Override it when your
         * protocol can enumerate (OPC UA browse, an EtherNet/IP tag list).
         */
        default BrowsePage browse(String cursor, int max) throws BrowseException {
            throw BrowseException.unsupported();
        }

        /** Close the connection. Must be safe to call twice. */
        default void close() {
        }
    }

    /** Opens sessions. One factory per protocol. */
    interface DeviceBackend {

        /** The protocol's name, as it appears in config and in the published {@code device.adapter} field. */
        String kind();

        /**
         * The signal inventory this backend exposes for a device, <b>without connecting</b> — read from
         * config in a real adapter. Backs {@code sb/signals} (a config view, no device round-trip). The
         * simulator returns a fixed pair so the command has something to show.
         */
        default List<SignalInfo> inventory(ConnectionConfig cfg) {
            return List.of();
        }

        /** Connect to one device. Throws {@link DeviceException} when unreachable or misconfigured. */
        DeviceSession connect(ConnectionConfig cfg) throws DeviceException;
    }

    // --- The simulated backend -------------------------------------------------------------------
    //
    // A real adapter replaces this with its protocol. It ships so that the component runs with no
    // hardware, and so the tests have something to talk to — and a backend you can run on a laptop is
    // worth more than one you can only run next to a PLC.

    /** The signals the simulator exposes — the ids it reads and the one it fails ({@code id, name, type}). */
    private static final String[][] SIM_SIGNALS = {
            {"temperature-1", "Ambient temperature", "REAL"},
            {"pressure-1", "Line pressure", "REAL"},
    };

    static final class SimBackend implements DeviceBackend {
        @Override
        public String kind() {
            return "sim";
        }

        @Override
        public List<SignalInfo> inventory(ConnectionConfig cfg) {
            List<SignalInfo> out = new ArrayList<>();
            for (String[] s : SIM_SIGNALS) {
                out.add(new SignalInfo(s[0], s[1]));
            }
            return out;
        }

        @Override
        public DeviceSession connect(ConnectionConfig cfg) throws DeviceException {
            if (cfg.endpoint() == null || cfg.endpoint().isEmpty()) {
                // A missing endpoint will never fix itself: permanent, so the supervisor does not
                // spend the next hour reconnecting to nothing.
                throw DeviceException.permanent("no endpoint configured");
            }
            return new SimSession();
        }
    }

    static final class SimSession implements DeviceSession {
        private static final Logger LOGGER = LogManager.getLogger(SimSession.class);
        private long tick = 0;

        @Override
        public List<Reading> readSignals() {
            tick++;
            double value = 20.0 + 5.0 * Math.sin(tick / 10.0);
            List<Reading> out = new ArrayList<>();
            out.add(new Reading("temperature-1", "Ambient temperature",
                    new JsonPrimitive(value), Quality.GOOD, "OK"));
            // A signal the simulated device cannot currently read. It is published as BAD rather than
            // omitted, because "I could not read this" is information and silence is not.
            out.add(new Reading("pressure-1", "Line pressure",
                    JsonNull.INSTANCE, Quality.BAD, "SENSOR_FAULT"));
            return out;
        }

        @Override
        public void writeSignal(String signalId, JsonElement value) {
            LOGGER.info("sim: write accepted for {} = {}", signalId, value);
        }

        /**
         * A one-page browse of the simulator's inventory. A real backend pages a large address space
         * and returns a {@code nextCursor}; the simulator has two signals, so the first page is the
         * last page.
         */
        @Override
        public BrowsePage browse(String cursor, int max) {
            // A cursor means "the page after the last one" — the sim has nothing more.
            if (cursor != null) {
                return BrowsePage.empty();
            }
            List<BrowsedSignal> entries = new ArrayList<>();
            for (String[] s : SIM_SIGNALS) {
                entries.add(new BrowsedSignal(s[0], s[1], s[2]));
            }
            return new BrowsePage(entries, null);
        }
    }
}
