package <<PACKAGE>>;

import com.google.gson.JsonElement;
import com.google.gson.JsonPrimitive;
import org.junit.jupiter.api.Test;

import java.util.ArrayList;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * The device-seam decision logic behind the session-only verbs — the session-null guard and the
 * per-verb exception-to-error-code remap for {@code sb/read}, {@code sb/write}, and {@code sb/browse}.
 * This is the branching the device worker used to hold inside its (excluded) run-loop class; it runs
 * against the pure {@link Device.DeviceSession} seam, so a fake session covers every branch with no
 * broker, no facade, and no live device.
 */
class ControlTest {

    /** A fake session, implementing the pure seam directly — the same trick {@code SimSession} uses. */
    private static final class FakeSession implements Device.DeviceSession {
        boolean fail = false;
        Device.BrowseException browseError = null;
        final List<String> written = new ArrayList<>();

        @Override
        public List<Device.Reading> readSignals() throws Device.DeviceException {
            if (fail) {
                throw Device.DeviceException.transientError("link down");
            }
            List<Device.Reading> out = new ArrayList<>();
            out.add(new Device.Reading("temperature-1", null, new JsonPrimitive(42.0),
                    Device.Quality.GOOD, "OK"));
            return out;
        }

        @Override
        public List<Device.Reading> readNamed(List<String> ids) throws Device.DeviceException {
            if (fail) {
                throw Device.DeviceException.transientError("link down");
            }
            List<Device.Reading> out = new ArrayList<>();
            for (String id : ids) {
                out.add(new Device.Reading(id, null, new JsonPrimitive(1.0), Device.Quality.GOOD, "OK"));
            }
            return out;
        }

        @Override
        public void writeSignal(String signalId, JsonElement value) throws Device.DeviceException {
            if (fail) {
                throw Device.DeviceException.permanent("device rejected");
            }
            written.add(signalId);
        }

        @Override
        public Device.BrowsePage browse(String cursor, int max) throws Device.BrowseException {
            if (browseError != null) {
                throw browseError;
            }
            return new Device.BrowsePage(
                    List.of(new Device.BrowsedSignal("temperature-1", "Ambient temperature", "REAL")),
                    null);
        }
    }

    // --- sb/read -----------------------------------------------------------------------------------

    @Test
    void readNowReadsThenGuardsANullSessionAndRemapsALinkError() throws Exception {
        FakeSession s = new FakeSession();
        List<Device.Reading> out = Control.readNow(s, List.of("temperature-1"));
        assertEquals(1, out.size());
        assertEquals("temperature-1", out.get(0).signalId());

        // A disconnected worker (null session) is DEVICE_UNAVAILABLE, not a read failure.
        assertThrows(Commands.DeviceUnavailableException.class,
                () -> Control.readNow(null, List.of("temperature-1")));

        // A link error mid-read is remapped to READ_FAILED.
        s.fail = true;
        assertThrows(Commands.ReadFailedException.class,
                () -> Control.readNow(s, List.of("temperature-1")));
    }

    // --- sb/write ----------------------------------------------------------------------------------

    @Test
    void writeConfirmsThenGuardsANullSessionAndRemapsARejection() throws Exception {
        FakeSession s = new FakeSession();
        Control.write(s, "setpoint-1", new JsonPrimitive(42));
        assertEquals(List.of("setpoint-1"), s.written, "the write reached the device");

        // Null session → DEVICE_UNAVAILABLE, and nothing reaches the device.
        assertThrows(Commands.DeviceUnavailableException.class,
                () -> Control.write(null, "setpoint-1", new JsonPrimitive(42)));

        // A device rejection is remapped to WRITE_FAILED.
        s.fail = true;
        assertThrows(Commands.WriteFailedException.class,
                () -> Control.write(s, "setpoint-1", new JsonPrimitive(42)));
    }

    // --- sb/browse ---------------------------------------------------------------------------------

    @Test
    void browseReturnsAPageGuardsANullSessionAndPassesThroughBrowseErrors() throws Exception {
        FakeSession s = new FakeSession();
        Device.BrowsePage page = Control.browse(s, null, 10);
        assertEquals(1, page.entries().size());
        assertEquals("temperature-1", page.entries().get(0).id());

        // A disconnected device is a browse FAILURE (the link is down), never "unsupported".
        Device.BrowseException down = assertThrows(Device.BrowseException.class,
                () -> Control.browse(null, null, 10));
        assertFalse(down.isUnsupported(), "a down link is a failure, not a capability gap");

        // A protocol with no discovery service passes straight through as unsupported.
        s.browseError = Device.BrowseException.unsupported();
        assertTrue(assertThrows(Device.BrowseException.class,
                () -> Control.browse(s, null, 10)).isUnsupported());

        // A mid-browse failure passes straight through as a failure.
        s.browseError = Device.BrowseException.failed("mid-browse error");
        assertFalse(assertThrows(Device.BrowseException.class,
                () -> Control.browse(s, null, 10)).isUnsupported());
    }
}
