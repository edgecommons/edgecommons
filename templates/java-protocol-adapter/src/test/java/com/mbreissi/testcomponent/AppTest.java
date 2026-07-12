package <<PACKAGE>>;

import com.mbreissi.edgecommons.heartbeat.InstanceConnectivity;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Per-device connectivity — the report this adapter exists to produce.
 *
 * <p>Every entry the adapter builds here is served on <b>two</b> surfaces from a single provider:
 * the library pushes it in each {@code state} keepalive's {@code instances[]}, and returns the same
 * sample from the built-in {@code status} verb when an operator pulls. So these assertions are the
 * contract for both.
 */
class <<COMPONENTNAME>>Test {

    private static final String ENDPOINT = "opc.tcp://plc-1:4840";

    @Test
    void aConfiguredDeviceThatIsNotUpYetIsStillReported() {
        // The one that matters. A device that is configured but not connected must APPEAR, as
        // down — otherwise it is indistinguishable from a device nobody ever configured, and the
        // adapter silently under-reports exactly when an operator needs it most.
        InstanceConnectivity connecting =
                <<COMPONENTNAME>>.connectivity("plc-1", ENDPOINT, false, "CONNECTING", null);

        assertEquals("plc-1", connecting.getInstance());
        assertFalse(connecting.isConnected());
        assertEquals("CONNECTING", connecting.getState());
    }

    @Test
    void aConnectedDeviceReportsItsEndpoint() {
        InstanceConnectivity online =
                <<COMPONENTNAME>>.connectivity("plc-1", ENDPOINT, true, "ONLINE", null);

        assertTrue(online.isConnected(), "connected is the NORMALIZED flag every console reads");
        assertEquals("ONLINE", online.getState());
        assertEquals(ENDPOINT, online.getDetail(), "when up, the detail is where it is connected");
        assertEquals("example", online.getAttributes().get("adapter").getAsString(),
                "protocol-specific facts belong in the open attributes bag, never in the "
                        + "normalized fields every consumer relies on");
    }

    @Test
    void aFailedDeviceReportsWhyRatherThanWhere() {
        InstanceConnectivity down =
                <<COMPONENTNAME>>.connectivity("plc-1", ENDPOINT, false, "BACKOFF", "connect timed out");

        assertFalse(down.isConnected());
        assertEquals("connect timed out", down.getDetail(), "when down, the detail is the reason");
    }

    @Test
    void connectedIsNormalizedWhileStateCarriesTheRicherCondition() {
        // This is why `state` exists at all: a boolean cannot tell "has never reached the device
        // yet" from "was connected and is now retrying". Both are connected=false, and an operator
        // must be able to tell them apart.
        InstanceConnectivity connecting =
                <<COMPONENTNAME>>.connectivity("plc-1", ENDPOINT, false, "CONNECTING", null);
        InstanceConnectivity retrying =
                <<COMPONENTNAME>>.connectivity("plc-1", ENDPOINT, false, "BACKOFF", "connect timed out");

        assertEquals(connecting.isConnected(), retrying.isConnected());
        assertNotEquals(connecting.getState(), retrying.getState());
    }
}
