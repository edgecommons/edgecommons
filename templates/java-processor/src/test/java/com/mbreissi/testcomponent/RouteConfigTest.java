package <<PACKAGE>>;

import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

/** One route per instance, and the defaults that make the common case the easy one. */
class RouteConfigTest {

    private static JsonObject json(String s) {
        return JsonParser.parseString(s).getAsJsonObject();
    }

    @Test
    void aRouteParsesFromItsInstanceConfig() {
        RouteConfig route = RouteConfig.parse(json("""
                {
                  "id": "temps",
                  "subscribe": ["ecv1/+/+/+/data/#"],
                  "publishTopic": "ecv1/gw01/proc/main/data/rollup",
                  "target": "northbound",
                  "pipeline": [
                    { "fieldEquals": { "path": "signal.id", "value": "temp-1" } },
                    { "countPerTick": {} }
                  ],
                  "tickMs": 5000
                }
                """), null);

        assertEquals("temps", route.id());
        assertEquals(RouteConfig.Target.NORTHBOUND, route.target());
        assertEquals(2, route.pipeline().size());
        assertEquals(5_000, route.tickMs());
        assertEquals(RouteConfig.DEFAULT_MAX_QUEUE, route.maxQueue(), "the queue is bounded by default");
    }

    @Test
    void theDefaultsAreTheCommonCase() {
        RouteConfig route = RouteConfig.parse(json("{\"id\":\"r\",\"publishTopic\":\"t\"}"), null);
        assertEquals(RouteConfig.Target.LOCAL, route.target(), "the device-local bus is the common target");
        assertTrue(route.pipeline().isEmpty(), "no stages == a pass-through republisher");
        assertEquals(RouteConfig.DEFAULT_TICK_MS, route.tickMs());
    }

    @Test
    void componentGlobalDefaultsFillWhatTheRouteOmits() {
        JsonObject defaults = json("{\"tickMs\":2500,\"maxQueue\":16}");
        RouteConfig route = RouteConfig.parse(json("{\"id\":\"r\",\"publishTopic\":\"t\"}"), defaults);
        assertEquals(2_500, route.tickMs());
        assertEquals(16, route.maxQueue());

        RouteConfig overriding = RouteConfig.parse(
                json("{\"id\":\"r\",\"publishTopic\":\"t\",\"tickMs\":100}"), defaults);
        assertEquals(100, overriding.tickMs(), "the route wins over the global default");
    }

    @Test
    void anUnknownConfigKeyIsRejectedRatherThanIgnored() {
        // A typo'd route key is a mistake, not a no-op — Gson would silently drop it.
        assertThrows(IllegalArgumentException.class, () -> RouteConfig.parse(
                json("{\"id\":\"r\",\"publishTopic\":\"t\",\"pipelnie\":[]}"), null));
    }

    @Test
    void aMissingRequiredKeyIsRejected() {
        assertThrows(IllegalArgumentException.class,
                () -> RouteConfig.parse(json("{\"id\":\"r\"}"), null));
    }

    @Test
    void anUnknownStageIsRejectedRatherThanSilentlySkipped() {
        assertThrows(IllegalArgumentException.class, () -> RouteConfig.parse(
                json("{\"id\":\"r\",\"publishTopic\":\"t\",\"pipeline\":[{\"nosuch\":{}}]}"), null));
    }
}
