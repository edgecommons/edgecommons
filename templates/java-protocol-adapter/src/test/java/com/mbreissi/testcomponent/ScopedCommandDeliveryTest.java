package <<PACKAGE>>;

import com.mbreissi.edgecommons.commands.CommandInbox;
import com.mbreissi.edgecommons.config.ConfigManager;
import com.mbreissi.edgecommons.config.ConfigurationFactory;
import com.mbreissi.edgecommons.config.TagConfiguration;
import com.mbreissi.edgecommons.messaging.Message;
import com.mbreissi.edgecommons.messaging.MessageBuilder;
import com.mbreissi.edgecommons.messaging.MessageIdentity;
import com.mbreissi.edgecommons.messaging.MessagingClient;
import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import com.google.gson.JsonPrimitive;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

import java.time.Duration;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.function.BiConsumer;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * The {@code sb/*} surface driven END TO END through a REAL {@link CommandInbox} — the delivery
 * topic, the envelope, the library's scope enforcement, the handler, and the reply — over a
 * recording messaging seam (no broker, no device).
 *
 * <p>The unit tests in {@code CommandsTest} call the {@code Commander} directly, so they can only
 * prove what this component does with an <i>already resolved</i> instance. These tests prove the
 * half the library now owns: two devices are configured (so nothing resolves by default), and the
 * command still reaches the right one because the <b>delivery topic</b> addressed it. They also
 * pin the two rules the adapter must NOT re-implement — a body conflicting with the topic is
 * refused before dispatch (the handler never runs), and every verb advertises
 * {@code "scope": "instance"} in {@code describe}.
 */
class ScopedCommandDeliveryTest {

    private static final String REPLY_TO = "edgecommons/reply-scoped-test";
    private static final String COMPONENT_FILTER = "ecv1/test-thing/TestComponent/cmd/#";
    private static final String INSTANCE_FILTER = "ecv1/test-thing/TestComponent/+/cmd/#";

    /** The nine verbs this template registers, all of them instance-scoped. */
    private static final List<String> SB_VERBS = List.of(
            "sb/status", "sb/read", "sb/write", "sb/signals", "sb/browse",
            "sb/pause", "sb/resume", "reconnect", "repoll");

    // --- the two seams the inbox needs (identity + a transport that records) ----------------------

    /** Just enough {@link ConfigManager} for the inbox: a resolved UNS identity. */
    private static final class TestConfig extends ConfigManager {

        @Override
        public MessageIdentity getComponentIdentity() {
            return new MessageIdentity(
                    List.of(new MessageIdentity.HierEntry("device", "test-thing")),
                    "TestComponent", null);
        }

        @Override
        public String getThingName() {
            return "test-thing";
        }

        @Override
        public String getComponentName() {
            return "TestComponent";
        }

        @Override
        public String getComponentFullName() {
            return "com.example.TestComponent";
        }

        @Override
        public TagConfiguration getTagConfig() {
            return ConfigurationFactory.createTagConfiguration(null);
        }
    }

    /** Records the inbox's subscriptions and its replies instead of talking to a broker. */
    private static final class TestMessaging extends MessagingClient {

        final Map<String, BiConsumer<String, Message>> subscriptions = new LinkedHashMap<>();
        final List<Message> replies = new ArrayList<>();

        @Override
        public void subscribeAcknowledged(String topicFilter, BiConsumer<String, Message> callback,
                                          int maxConcurrency, int maxMessages, Duration timeout) {
            subscriptions.put(topicFilter, callback);
        }

        @Override
        public void unsubscribe(String topicFilter) {
            subscriptions.remove(topicFilter);
        }

        @Override
        public void reply(Message request, Message reply) {
            replies.add(reply);
        }

        @Override
        public boolean connected() {
            return true;
        }
    }

    /** A control seam that only records: no verb under test needs real device I/O. */
    private static final class RecordingControl implements Commands.DeviceControl {

        private final Health health;
        final List<String> calls = new ArrayList<>();

        RecordingControl(Health health) {
            this.health = health;
        }

        @Override
        public List<Device.Reading> readNow(List<String> ids) {
            calls.add("readNow");
            List<Device.Reading> out = new ArrayList<>();
            for (String id : ids) {
                out.add(new Device.Reading(id, null, new JsonPrimitive(1.0), Device.Quality.GOOD, "OK"));
            }
            return out;
        }

        @Override
        public void write(String signalId, JsonElement value) {
            calls.add("write");
        }

        @Override
        public Device.BrowsePage browse(String cursor, int max) {
            calls.add("browse");
            return new Device.BrowsePage(List.of(), null);
        }

        @Override
        public boolean pause() {
            calls.add("pause");
            return Wiring.setPaused(health, true);
        }

        @Override
        public boolean resume() {
            calls.add("resume");
            return Wiring.setPaused(health, false);
        }

        @Override
        public void reconnect() {
            calls.add("reconnect");
        }

        @Override
        public long repoll() {
            calls.add("repoll");
            return 1;
        }
    }

    // --- fixture -----------------------------------------------------------------------------------

    private TestMessaging messaging;
    private CommandInbox inbox;
    private final Map<String, Health> health = new LinkedHashMap<>();
    private final Map<String, RecordingControl> controls = new LinkedHashMap<>();

    @BeforeEach
    void setUp() {
        // TWO devices: nothing resolves by default, so every routing assertion below is proof that
        // the ADDRESSING (topic or body) carried the instance - not a single-device fallback.
        List<Commands.DeviceHandle> handles = new ArrayList<>();
        for (String id : List.of("plc-1", "plc-2")) {
            DeviceConfig cfg = new DeviceConfig(id, "sim",
                    new Device.ConnectionConfig("sim://" + id, new JsonObject()), 5_000L,
                    new Writes(List.of("setpoint-1")));
            Health h = new Health();
            h.setLink(LinkState.ONLINE);
            RecordingControl control = new RecordingControl(h);
            health.put(id, h);
            controls.put(id, control);
            handles.add(new Commands.DeviceHandle(cfg, control, h,
                    new DeviceMetrics(null, null, id, h, 30, 1),
                    List.of(new Device.SignalInfo("temperature-1", "Ambient temperature"))));
        }

        messaging = new TestMessaging();
        inbox = new CommandInbox(new TestConfig(), messaging, () -> 42L, () -> true, JsonObject::new);
        Commands.registerAll(inbox, handles);
        inbox.start(Duration.ofSeconds(5));
        assertEquals(2, messaging.subscriptions.size(),
                "the inbox subscribes both D-U28 cmd wildcards");
    }

    @AfterEach
    void tearDown() {
        inbox.close();
    }

    // --- driving one command through the real inbox ------------------------------------------------

    /** Delivers a request on {@code topic} and returns the single reply body. */
    private JsonObject deliver(String topic, String verb, String bodyJson) {
        messaging.replies.clear();
        Message request = MessageBuilder.create(verb, "1.0")
                .withPayload(JsonParser.parseString(bodyJson).getAsJsonObject())
                .build();
        request.makeRequest(REPLY_TO);
        String filter = topic.startsWith("ecv1/test-thing/TestComponent/cmd/")
                ? COMPONENT_FILTER : INSTANCE_FILTER;
        messaging.subscriptions.get(filter).accept(topic, request);
        assertEquals(1, messaging.replies.size(), "exactly one reply expected for " + topic);
        return messaging.replies.get(0).toDict().getAsJsonObject("body");
    }

    private static JsonObject okResult(JsonObject reply) {
        assertTrue(reply.get("ok").getAsBoolean(), "expected a success reply, got: " + reply);
        return reply.getAsJsonObject("result");
    }

    private static String errorCode(JsonObject reply) {
        assertFalse(reply.get("ok").getAsBoolean(), "expected an error reply, got: " + reply);
        return reply.getAsJsonObject("error").get("code").getAsString();
    }

    private static String errorMessage(JsonObject reply) {
        return reply.getAsJsonObject("error").get("message").getAsString();
    }

    // --- the addressing the library resolves --------------------------------------------------------

    @Test
    void anInstanceAddressedTopicRoutesWithNoInstanceInTheBody() {
        JsonObject result = okResult(
                deliver("ecv1/test-thing/TestComponent/plc-2/cmd/sb/status", "sb/status", "{}"));
        assertEquals("plc-2", result.get("id").getAsString(),
                "the delivery topic addressed plc-2, and two devices are configured");
    }

    @Test
    void aComponentAddressedTopicStillHonorsAnInstanceNamedInTheBody() {
        JsonObject result = okResult(
                deliver("ecv1/test-thing/TestComponent/cmd/sb/status", "sb/status",
                        "{\"instance\":\"plc-1\"}"));
        assertEquals("plc-1", result.get("id").getAsString(),
                "the library folds a body-named instance into the addressed instance");
    }

    @Test
    void aComponentAddressedCommandNamingNoInstanceIsBadArgsWithTwoDevices() {
        JsonObject reply = deliver("ecv1/test-thing/TestComponent/cmd/sb/status", "sb/status", "{}");
        assertEquals("BAD_ARGS", errorCode(reply));
        assertEquals("field `instance` is required when multiple devices are configured",
                errorMessage(reply), "the component-side default policy answers, not the library");
    }

    @Test
    void anUnknownAddressedInstanceIsNoSuchInstance() {
        JsonObject reply =
                deliver("ecv1/test-thing/TestComponent/ghost/cmd/sb/status", "sb/status", "{}");
        assertEquals("NO_SUCH_INSTANCE", errorCode(reply));
    }

    @Test
    void aBodyConflictingWithTheTopicIsRefusedByTheLibraryAndTheHandlerNeverRuns() {
        JsonObject reply = deliver("ecv1/test-thing/TestComponent/plc-1/cmd/sb/pause", "sb/pause",
                "{\"instance\":\"plc-2\"}");
        assertEquals("BAD_ARGS", errorCode(reply));
        assertEquals("instance in body conflicts with the addressed instance", errorMessage(reply));

        // The proof that the rejection happened BEFORE dispatch: neither device was paused, and the
        // control seam was never touched. This adapter has no conflict check of its own to run.
        assertFalse(health.get("plc-1").isPaused());
        assertFalse(health.get("plc-2").isPaused());
        assertTrue(controls.get("plc-1").calls.isEmpty());
        assertTrue(controls.get("plc-2").calls.isEmpty());
    }

    @Test
    void anInstanceAddressedLifecycleVerbActsOnThatDeviceOnly() {
        JsonObject result = okResult(
                deliver("ecv1/test-thing/TestComponent/plc-1/cmd/sb/pause", "sb/pause", "{}"));
        assertEquals("plc-1", result.get("id").getAsString());
        assertTrue(result.get("paused").getAsBoolean());
        assertTrue(health.get("plc-1").isPaused());
        assertFalse(health.get("plc-2").isPaused(), "the other device is untouched");
    }

    // --- what describe advertises -------------------------------------------------------------------

    @Test
    void describeAdvertisesEverySbVerbAsInstanceScoped() {
        JsonObject result = okResult(
                deliver("ecv1/test-thing/TestComponent/cmd/describe", "describe", "{}"));
        JsonArray commands = result.getAsJsonArray("commands");
        Map<String, JsonObject> byVerb = new LinkedHashMap<>();
        for (JsonElement e : commands) {
            byVerb.put(e.getAsJsonObject().get("verb").getAsString(), e.getAsJsonObject());
        }
        for (String verb : SB_VERBS) {
            JsonObject entry = byVerb.get(verb);
            assertTrue(entry != null, "describe is missing the verb " + verb);
            assertEquals("instance", entry.get("scope").getAsString(),
                    verb + " is registered instance-scoped");
            assertFalse(entry.get("builtIn").getAsBoolean());
        }
        // The library's own verbs answer at either scope.
        assertEquals("both", byVerb.get("ping").get("scope").getAsString());
    }
}
