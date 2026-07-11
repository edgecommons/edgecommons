package <<PACKAGE>>;

import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import com.google.gson.JsonPrimitive;
import com.mbreissi.edgecommons.messaging.Message;
import com.mbreissi.edgecommons.messaging.MessageBuilder;
import org.junit.jupiter.api.Test;

import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

/** The stage contract: 0..N out, and time-driven output through {@code onTick}. */
class PipelineTest {

    private static ProcMsg msg(String json) {
        JsonObject body = JsonParser.parseString(json).getAsJsonObject();
        Message m = MessageBuilder.create("T", "1.0").withPayload(body).build();
        return new ProcMsg("ecv1/gw/x/main/data/t", m);
    }

    private static JsonObject bodyOf(ProcMsg m) {
        return (JsonObject) m.msg().getBody();
    }

    @Test
    void aFilterStageDropsWhatDoesNotMatch() {
        Pipeline p = new Pipeline(List.of(
                new Stages.FieldEquals("quality", new JsonPrimitive("GOOD"))));

        assertEquals(1, p.run(List.of(msg("{\"quality\":\"GOOD\"}")), null).size());
        assertTrue(p.run(List.of(msg("{\"quality\":\"BAD\"}")), null).isEmpty(),
                "a filter that does not match emits nothing");
    }

    @Test
    void aStatefulStageEmitsOnTheTickNotOnArrival() {
        Pipeline p = new Pipeline(List.of(new Stages.CountPerTick()));

        // Three messages arrive: nothing goes downstream yet.
        for (int i = 0; i < 3; i++) {
            assertTrue(p.run(List.of(msg("{\"v\":1}")), null).isEmpty());
        }
        // The tick closes the window and emits one rollup.
        List<ProcMsg> out = p.run(List.of(), 1_000L);
        assertEquals(1, out.size());
        assertEquals(3, bodyOf(out.get(0)).get("count").getAsInt());

        // A second tick with nothing accumulated emits nothing — an empty window is not an event.
        assertTrue(p.run(List.of(), 2_000L).isEmpty());
    }

    @Test
    void stagesChainAndATickFlowsThroughTheRestOfThePipeline() {
        // Filter, then count. A window closing in stage 1 is projected by stage 2 on the same pass.
        Pipeline p = new Pipeline(List.of(
                new Stages.FieldEquals("quality", new JsonPrimitive("GOOD")),
                new Stages.CountPerTick()));

        p.run(List.of(msg("{\"quality\":\"GOOD\"}")), null);
        p.run(List.of(msg("{\"quality\":\"BAD\"}")), null); // filtered out
        List<ProcMsg> out = p.run(List.of(), 1_000L);

        assertEquals(1, out.size());
        assertEquals(1, bodyOf(out.get(0)).get("count").getAsInt(),
                "only the GOOD message reached the counter");
    }

    @Test
    void anEmptyPipelineIsAPassThroughRepublisher() {
        Pipeline p = new Pipeline(List.of());
        List<ProcMsg> out = p.run(List.of(msg("{\"v\":1}")), null);
        assertEquals(1, out.size());
        assertEquals(1, bodyOf(out.get(0)).get("v").getAsInt());
    }

    @Test
    void pluckWalksADottedPath() {
        JsonObject body = JsonParser.parseString("{\"signal\":{\"id\":\"temp-1\"}}").getAsJsonObject();
        assertEquals("temp-1", Stages.pluck(body, "signal.id").getAsString());
        assertNull(Stages.pluck(body, "signal.nope"));
        assertNull(Stages.pluck("not json", "signal.id"));
    }
}
