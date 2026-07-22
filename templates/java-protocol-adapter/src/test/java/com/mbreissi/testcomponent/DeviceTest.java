package <<PACKAGE>>;

import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import org.junit.jupiter.api.Test;

import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * The device seam and its simulated backend: what a protocol adapter talks to, running on a laptop
 * with no hardware. A real adapter replaces {@link Device.SimBackend} and these tests keep the same
 * shape.
 */
class DeviceTest {

    private static Device.ConnectionConfig conn(String endpoint) {
        return new Device.ConnectionConfig(endpoint, new JsonObject());
    }

    @Test
    void theSimBackendConnectsAndReads() throws Exception {
        Device.DeviceSession s = new Device.SimBackend().connect(conn("sim://device"));
        List<Device.Reading> readings = s.readSignals();
        assertEquals(2, readings.size());
        assertEquals("temperature-1", readings.get(0).signalId());
        assertEquals(Device.Quality.GOOD, readings.get(0).quality());
    }

    @Test
    void aFailedReadIsPublishedAsBadQualityNotOmitted() throws Exception {
        // The signal is still reported — with BAD quality and the native code — because a signal that
        // silently vanishes is indistinguishable from one that is not changing.
        Device.DeviceSession s = new Device.SimBackend().connect(conn("sim://device"));
        Device.Reading bad = s.readSignals().stream()
                .filter(r -> r.signalId().equals("pressure-1")).findFirst().orElseThrow();
        assertEquals(Device.Quality.BAD, bad.quality());
        assertEquals("SENSOR_FAULT", bad.qualityRaw());
    }

    @Test
    void aMisconfigurationIsPermanentSoTheSupervisorDoesNotHammerIt() {
        Device.DeviceException e = assertThrows(Device.DeviceException.class,
                () -> new Device.SimBackend().connect(conn("")));
        assertFalse(e.isTransient(), "a missing endpoint will never fix itself by retrying");
    }

    @Test
    void readingsAdvance() throws Exception {
        Device.DeviceSession s = new Device.SimBackend().connect(conn("sim://device"));
        JsonElement a = s.readSignals().get(0).value();
        JsonElement b = s.readSignals().get(0).value();
        assertNotEquals(a, b);
    }

    @Test
    void readNamedReturnsOnlyTheRequestedSignals() throws Exception {
        // The default readNamed reads all and filters — override it only if your protocol reads a
        // subset more cheaply.
        Device.DeviceSession s = new Device.SimBackend().connect(conn("sim://device"));
        List<Device.Reading> got = s.readNamed(List.of("temperature-1"));
        assertEquals(1, got.size());
        assertEquals("temperature-1", got.get(0).signalId());
        // An unknown id resolves to nothing (the command layer reports it as a BAD/no-data entry).
        assertTrue(s.readNamed(List.of("nope")).isEmpty());
    }

    @Test
    void theSimBrowsesOnePageAndStops() throws Exception {
        Device.DeviceSession s = new Device.SimBackend().connect(conn("sim://device"));
        Device.BrowsePage page = s.browse(null, 100);
        assertEquals(2, page.entries().size());
        assertEquals("temperature-1", page.entries().get(0).id());
        assertEquals(null, page.nextCursor(), "the sim's first page is its last");
        // A cursor asks for the page after the last — empty.
        assertTrue(s.browse("x", 100).entries().isEmpty());
    }

    @Test
    void theSimAdvertisesItsInventoryWithoutConnecting() {
        // sb/signals reads this — a config view, no device round-trip.
        List<Device.SignalInfo> inv = new Device.SimBackend().inventory(conn("sim://device"));
        assertEquals(2, inv.size());
        assertEquals("temperature-1", inv.get(0).id());
        assertEquals("Ambient temperature", inv.get(0).name());
    }

    @Test
    void browseIsUnsupportedByDefault() {
        // A protocol with no discovery keeps the default — honest, not a fake empty page.
        Device.DeviceSession noBrowse = new Device.DeviceSession() {
            @Override
            public List<Device.Reading> readSignals() {
                return List.of();
            }

            @Override
            public void writeSignal(String signalId, JsonElement value) {
            }
        };
        Device.BrowseException e = assertThrows(Device.BrowseException.class,
                () -> noBrowse.browse(null, 10));
        assertTrue(e.isUnsupported());
    }
}
