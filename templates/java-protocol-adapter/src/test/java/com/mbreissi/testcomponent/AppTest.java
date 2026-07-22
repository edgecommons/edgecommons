package <<PACKAGE>>;

import com.mbreissi.edgecommons.config.ConfigManager;
import com.mbreissi.edgecommons.heartbeat.InstanceConnectivity;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * The adapter's config model, backoff, and the per-device connectivity report it exists to produce.
 *
 * <p>Every connectivity entry is served on <b>two</b> surfaces from a single provider: the library
 * pushes it in each {@code state} keepalive's {@code instances[]}, and returns the same sample from the
 * built-in {@code status} verb. So these assertions are the contract for both.
 */
class <<COMPONENTNAME>>Test {

    private static JsonObject json(String s) {
        return JsonParser.parseString(s).getAsJsonObject();
    }

    @Test
    void aDeviceParsesFromItsInstanceConfig() {
        DeviceConfig d = DeviceConfig.from(json(
                "{\"id\":\"plc-1\",\"adapter\":\"sim\",\"connection\":{\"endpoint\":\"sim://plc-1\","
                        + "\"unitId\":3},\"pollIntervalMs\":1000,\"writes\":{\"allow\":[\"setpoint-1\"]}}"));

        assertEquals("plc-1", d.id());
        assertEquals(1000L, d.pollIntervalMs());
        assertEquals("sim://plc-1", d.connection().endpoint());
        // `connection` is deliberately open: every protocol needs different keys.
        assertEquals(3, d.connection().extra().get("unitId").getAsInt());
        assertTrue(d.writes().permits("setpoint-1"));
    }

    @Test
    void anAdapterIsReadOnlyUntilAWriteIsAllowListed() {
        // The default must be read-only. An adapter that writes any address it is asked to is a
        // control-system vulnerability, not a convenience.
        DeviceConfig d = DeviceConfig.from(json(
                "{\"id\":\"plc-1\",\"connection\":{\"endpoint\":\"sim://plc-1\"}}"));
        assertFalse(d.writes().permits("setpoint-1"), "nothing is writable by default");
        assertEquals("sim", d.adapter(), "adapter defaults to the simulator");

        Writes w = new Writes(java.util.List.of("setpoint-1"));
        assertTrue(w.permits("setpoint-1"));
        assertFalse(w.permits("setpoint-2"), "only the listed signal, not its neighbours");
    }

    @Test
    void reconnectBackoffIsExponentialCappedAndJittered() {
        Backoff b = new Backoff(1_000, 10_000);
        assertEquals(1_000, b.delayMs(0, 1.0));
        assertEquals(4_000, b.delayMs(2, 1.0));
        assertEquals(10_000, b.delayMs(20, 1.0), "capped");
        // Jitter: the delay is a point in the window, not its edge.
        assertEquals(2_000, b.delayMs(2, 0.5));
        assertEquals(0, b.delayMs(2, 0.0));
    }

    @Test
    void everyDeviceReportsItsOwnConnectivity() {
        DeviceConfig cfg = DeviceConfig.from(json(
                "{\"id\":\"plc-1\",\"adapter\":\"sim\",\"connection\":{\"endpoint\":\"sim://plc-1\"}}"));
        Health health = new Health();

        // Before the first connect: not reachable, and the token says why — CONNECTING, not BACKOFF.
        InstanceConnectivity c = Wiring.connectivityOf(cfg, health);
        assertEquals("plc-1", c.getInstance());
        assertFalse(c.isConnected());
        assertEquals("CONNECTING", c.getState());
        assertEquals("sim://plc-1", c.getDetail(), "the endpoint, for a human");
        assertEquals("sim", c.getAttributes().get("adapter").getAsString(),
                "the open bag carries domain data");
        assertFalse(c.getAttributes().get("paused").getAsBoolean());

        health.setLink(LinkState.ONLINE);
        c = Wiring.connectivityOf(cfg, health);
        assertTrue(c.isConnected(), "the normalized flag every console reads");
        assertEquals("ONLINE", c.getState());

        health.setLink(LinkState.BACKOFF);
        assertFalse(Wiring.connectivityOf(cfg, health).isConnected());
    }

    @Test
    void aPausedOnlineDeviceReportsPausedButStaysConnected() {
        DeviceConfig cfg = DeviceConfig.from(json(
                "{\"id\":\"plc-1\",\"connection\":{\"endpoint\":\"sim://plc-1\"}}"));
        Health health = new Health();
        health.setLink(LinkState.ONLINE);

        assertTrue(Wiring.setPaused(health, true), "pausing changed the state");
        assertFalse(Wiring.setPaused(health, true), "pausing again is idempotent");
        InstanceConnectivity c = Wiring.connectivityOf(cfg, health);
        assertEquals("PAUSED", c.getState(), "paused + online = PAUSED");
        assertTrue(c.isConnected(), "connected stays truthful while paused");
        assertTrue(c.getAttributes().get("paused").getAsBoolean());

        // A break while paused reports BACKOFF (not PAUSED), connected false.
        health.setLink(LinkState.BACKOFF);
        c = Wiring.connectivityOf(cfg, health);
        assertEquals("BACKOFF", c.getState());
        assertFalse(c.isConnected());
    }

    @Test
    void theNormalizedFlagAndTheHealthMetricCannotDisagree() {
        Health health = new Health();
        health.setLink(LinkState.ONLINE);
        assertEquals(1, health.connectionState());
        health.setLink(LinkState.BACKOFF);
        assertEquals(0, health.connectionState());
    }

    // --- staleSignalSecs config read (Wiring) ------------------------------------------------------

    /** A minimal {@link ConfigManager} that returns a fixed {@code global} config (the only method
        {@link Wiring#readStaleSignalSecs} touches). Null/throwing global exercise the fallback. */
    private static ConfigManager configWithGlobal(JsonObject global, boolean throwOnRead) {
        return new ConfigManager() {
            @Override
            public JsonObject getGlobalConfig() {
                if (throwOnRead) {
                    throw new IllegalStateException("config unavailable");
                }
                return global;
            }
        };
    }

    @Test
    void staleSignalSecsIsReadFromConfigAndDefaultsWhenAbsentOrMalformed() {
        // An explicit healthThresholds.staleSignalSecs wins.
        assertEquals(45L, Wiring.readStaleSignalSecs(
                configWithGlobal(json("{\"healthThresholds\":{\"staleSignalSecs\":45}}"), false)));

        // No global config at all → the SOUTHBOUND.md §4/§5 default.
        assertEquals(Wiring.DEFAULT_STALE_SIGNAL_SECS,
                Wiring.readStaleSignalSecs(configWithGlobal(null, false)));

        // Present but malformed (healthThresholds is not an object) → default, not a crash.
        assertEquals(Wiring.DEFAULT_STALE_SIGNAL_SECS, Wiring.readStaleSignalSecs(
                configWithGlobal(json("{\"healthThresholds\":\"nonsense\"}"), false)));

        // A config read that throws is swallowed and defaulted — a threshold lookup must never take
        // the adapter down.
        assertEquals(Wiring.DEFAULT_STALE_SIGNAL_SECS,
                Wiring.readStaleSignalSecs(configWithGlobal(null, true)));
    }
}
