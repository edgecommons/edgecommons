package <<PACKAGE>>;

import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.condition.EnabledIfEnvironmentVariable;

import java.util.List;

import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;

/**
 * Live integration test against a real simulator or device — gated, and self-skipping.
 *
 * <p>This is deliberately <b>not</b> named {@code *Test.java}, so Surefire's default include
 * patterns never pick it up in a normal {@code mvn test}: the org's scaffold-build validation gate
 * runs the unit suite with no live infrastructure and this file plays no part in it unless invoked
 * explicitly. Point {@code EC_LIVE_SIM} at a running device/simulator endpoint and run it with
 * {@code mvn test -Dtest=LiveSimIT} (see {@code docs/how-to-guides.md}); every sibling reference
 * adapter (the permanent {@code ggcommons-modbus-sim} container, the EtherNet/IP cpppo/OpENer
 * simulators) gates its own live suite the same way.
 *
 * <p>The test body below talks to the scaffold's own {@link Device.SimBackend} so this file compiles
 * and demonstrates the shape out of the box. Once {@link Device} talks to a real protocol, point this
 * at that protocol instead — connect, run one poll cycle, and assert on the readings and quality your
 * real device actually returns.
 */
@EnabledIfEnvironmentVariable(named = "EC_LIVE_SIM", matches = ".+")
class LiveSimIT {

    @Test
    void connectsPollsOnceAndReportsReadingsWithQuality() throws Exception {
        String endpoint = System.getenv("EC_LIVE_SIM");
        Device.DeviceBackend backend = Device.backendFor("sim");
        assertNotNull(backend, "no backend registered for `sim` — update backendFor() alongside your protocol");

        Device.DeviceSession session = backend.connect(new Device.ConnectionConfig(endpoint, new com.google.gson.JsonObject()));
        try {
            List<Device.Reading> readings = session.readSignals();
            assertFalse(readings.isEmpty(), "one poll cycle against " + endpoint + " returned no readings");
            for (Device.Reading r : readings) {
                assertNotNull(r.quality(), "every reading must carry a quality, GOOD/BAD/UNCERTAIN");
            }
        } finally {
            session.close();
        }
    }
}
