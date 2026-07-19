package <<PACKAGE>>;

import com.mbreissi.edgecommons.commands.CommandException;
import com.mbreissi.edgecommons.messaging.Message;
import com.mbreissi.edgecommons.messaging.MessageBuilder;
import com.google.gson.JsonObject;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * The connectivity this component reports, and the {@code set-greeting} command handler.
 *
 * <p>A service owns no southbound connections, so it reports <b>no instances</b> — and that is the
 * contract, not an omission: the {@code state} keepalive carries no {@code instances[]} section, and
 * the built-in {@code status} verb therefore answers exactly as {@code ping} does
 * ({@code {"status":"RUNNING","uptimeSecs":n}}). Pinned here so it stays deliberate: the day this
 * component acquires a connection, this test is what tells you to report it.
 *
 * <p>The one piece of real logic — the {@code set-greeting} verb's parse/validate/swap — lives in
 * {@link Greeting}, out of the live run loop, so it is unit-tested here without a broker.
 */
class <<COMPONENTNAME>>Test {

    private static Message body(JsonObject payload) {
        return MessageBuilder.create("set-greeting", "1.0").withPayload(payload).build();
    }

    private static JsonObject greeting(String value) {
        JsonObject o = new JsonObject();
        o.addProperty("greeting", value);
        return o;
    }

    @Test
    void aComponentWithNoConnectionsReportsNoInstances() {
        assertTrue(<<COMPONENTNAME>>.instanceConnectivity().isEmpty());
    }

    @Test
    void theGreetingStartsAtItsInitialValue() {
        assertEquals("Hi", new Greeting("Hi").get());
    }

    @Test
    void setGreetingSwapsTheValueAndReportsThePrevious() throws CommandException {
        Greeting g = new Greeting("first");

        JsonObject reply = g.apply(body(greeting("second")));

        assertEquals("first", reply.get("previousGreeting").getAsString(),
                "the reply echoes what it replaced, so a console sees the effect");
        assertEquals("second", reply.get("greeting").getAsString());
        assertEquals("second", g.get(), "the next status tick reflects the new greeting");
    }

    @Test
    void aMissingOrMalformedGreetingIsBadArgs() {
        Greeting g = new Greeting("start");

        // No `greeting` field at all.
        assertEquals("BAD_ARGS", codeOf(g, body(new JsonObject())));
        // Present but not a primitive (an object, not a string/number).
        JsonObject nested = new JsonObject();
        nested.add("greeting", new JsonObject());
        assertEquals("BAD_ARGS", codeOf(g, body(nested)));
        // A non-JSON body is rejected the same way — the state is untouched.
        assertEquals("BAD_ARGS", codeOf(g, MessageBuilder.create("set-greeting", "1.0")
                .withPayload("not-json").build()));
        assertEquals("start", g.get(), "a rejected command must not mutate the greeting");
    }

    private static String codeOf(Greeting g, Message request) {
        try {
            g.apply(request);
        } catch (CommandException e) {
            return e.getCode();
        }
        throw new AssertionError("expected the command to fail with BAD_ARGS");
    }
}
