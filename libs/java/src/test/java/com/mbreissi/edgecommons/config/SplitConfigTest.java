package com.mbreissi.edgecommons.config;

import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import com.mbreissi.edgecommons.ParsedCommandLine;
import com.mbreissi.edgecommons.messaging.Message;
import com.mbreissi.edgecommons.messaging.MessageBuilder;
import com.mbreissi.edgecommons.messaging.MessagingClient;
import com.mbreissi.edgecommons.messaging.ReplyFuture;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import java.util.function.BiConsumer;

import static org.junit.jupiter.api.Assertions.*;

class SplitConfigTest {
    private static final String COMPONENT = "com.test.SplitComponent";
    private static final String THING = "gw-01";

    private static JsonObject parse(String json) {
        return JsonParser.parseString(json).getAsJsonObject();
    }

    private static Path write(Path dir, String name, String json) throws IOException {
        Path path = dir.resolve(name);
        Files.writeString(path, json, StandardCharsets.UTF_8);
        return path;
    }

    private static ParsedCommandLine fileCmd(Path config, boolean noSharedConfig) {
        ParsedCommandLine cmdLine = new ParsedCommandLine();
        cmdLine.configArgs = new String[]{"FILE", config.toString()};
        cmdLine.thingName = THING;
        cmdLine.noSharedConfig = noSharedConfig;
        return cmdLine;
    }

    @Test
    void mergeVectorsProduceExpectedEffectiveConfigs() throws IOException {
        Path vectors = Path.of("..", "..", "split-config-test-vectors", "merge.json");
        JsonArray cases = JsonParser.parseString(Files.readString(vectors)).getAsJsonObject()
                .getAsJsonArray("cases");

        for (JsonElement element : cases) {
            JsonObject testCase = element.getAsJsonObject();
            String name = testCase.get("name").getAsString();
            JsonObject expected = testCase.getAsJsonObject("expected");
            if (expected.has("error")) {
                continue;
            }
            JsonObject input = testCase.getAsJsonObject("input");
            JsonObject component = input.getAsJsonObject("component");
            boolean skipBase = input.has("options")
                    && input.getAsJsonObject("options").has("noSharedConfig")
                    && input.getAsJsonObject("options").get("noSharedConfig").getAsBoolean();
            skipBase = skipBase || (component.has("sharedConfig")
                    && component.get("sharedConfig").isJsonPrimitive()
                    && component.get("sharedConfig").getAsBoolean() == false);

            List<JsonObject> layers = new ArrayList<>();
            if (!skipBase && input.has("base") && input.get("base").isJsonObject()) {
                layers.add(input.getAsJsonObject("base"));
            }
            layers.add(component);
            JsonObject actual = DeepMerge.merge(layers);
            assertEquals(expected.get("effective"), actual, name);
            assertFalse(actual.has("extends"), name + " must strip extends");
            assertFalse(actual.has("sharedConfig"), name + " must strip sharedConfig");
        }
    }

    @Test
    void fileExtendsMergesBaseAndComponentThenStoresOnlyEffectiveConfig(@TempDir Path dir)
            throws Exception {
        write(dir, "shared.json", """
                {"hierarchy":{"levels":["site","device"]},
                 "identity":{"site":"dallas"},
                 "logging":{"level":"INFO"},
                 "tags":{"site":"dallas"}}""");
        Path component = write(dir, "config.json", """
                {"extends":"shared.json",
                 "sharedConfig":true,
                 "logging":{"level":"DEBUG"},
                 "component":{"global":{"v":1}}}""");

        ConfigManager cm = ConfigManagerFactory.create(COMPONENT, fileCmd(component, false));
        try {
            JsonObject effective = cm.getFullConfig();
            assertEquals("DEBUG", effective.getAsJsonObject("logging").get("level").getAsString());
            assertEquals("dallas", effective.getAsJsonObject("identity").get("site").getAsString());
            assertFalse(effective.has("extends"));
            assertFalse(effective.has("sharedConfig"));
            assertEquals("dallas", cm.getComponentIdentity().getHier().get(0).value());
        } finally {
            cm.close();
        }
    }

    @Test
    void sharedConfigFalseSkipsBaseEvenWhenExtendsIsPresent(@TempDir Path dir) throws Exception {
        write(dir, "shared.json", "{\"logging\":{\"level\":\"INFO\"}}");
        Path component = write(dir, "config.json", """
                {"extends":"shared.json",
                 "sharedConfig":false,
                 "component":{"global":{"v":1}}}""");

        ConfigManager cm = ConfigManagerFactory.create(COMPONENT, fileCmd(component, false));
        try {
            assertFalse(cm.getFullConfig().has("logging"));
            assertFalse(cm.getFullConfig().has("sharedConfig"));
        } finally {
            cm.close();
        }
    }

    @Test
    void cliNoSharedConfigWinsOverComponentSharedConfigTrue(@TempDir Path dir) throws Exception {
        write(dir, "shared.json", "{\"logging\":{\"level\":\"INFO\"}}");
        Path component = write(dir, "config.json", """
                {"extends":"shared.json",
                 "sharedConfig":true,
                 "component":{"global":{"v":1}}}""");

        ConfigManager cm = ConfigManagerFactory.create(COMPONENT, fileCmd(component, true));
        try {
            assertFalse(cm.getFullConfig().has("logging"));
        } finally {
            cm.close();
        }
    }

    @Test
    void baseExtendsIsRejectedAsNLayerInheritance(@TempDir Path dir) throws IOException {
        write(dir, "shared.json", "{\"extends\":\"site.json\",\"logging\":{\"level\":\"INFO\"}}");
        Path component = write(dir, "config.json", """
                {"extends":"shared.json","component":{"global":{}}}""");

        ConfigurationException ex = assertThrows(ConfigurationException.class,
                () -> ConfigManagerFactory.create(COMPONENT, fileCmd(component, false)));
        assertTrue(ex.getMessage().contains("N-layer")
                || (ex.getCause() != null && ex.getCause().getMessage().contains("N-layer")));
    }

    @Test
    void reloadRejectsInvalidMergedConfigAndKeepsPrevious(@TempDir Path dir) throws Exception {
        Path shared = write(dir, "shared.json", "{\"logging\":{\"level\":\"INFO\"}}");
        Path component = write(dir, "config.json", """
                {"extends":"shared.json","component":{"global":{"v":1}}}""");
        ConfigManager cm = ConfigManagerFactory.create(COMPONENT, fileCmd(component, false));
        cm.completeInitialization();
        try {
            Files.writeString(shared, "{\"bogusSection\":true}", StandardCharsets.UTF_8);
            assertFalse(cm.reloadFromProvider());
            assertEquals(1, cm.getFullConfig().getAsJsonObject("component")
                    .getAsJsonObject("global").get("v").getAsInt());
        } finally {
            cm.close();
        }
    }

    @Test
    void configMapUsesMountedSharedJsonDefault(@TempDir Path mount) throws Exception {
        write(mount, "shared.json", "{\"logging\":{\"level\":\"INFO\"}}");
        write(mount, "config.json", "{\"component\":{\"global\":{\"v\":2}}}");
        ParsedCommandLine cmdLine = new ParsedCommandLine();
        cmdLine.configArgs = new String[]{"CONFIGMAP", mount.toString(), "config.json"};
        cmdLine.thingName = THING;

        ConfigManager cm = ConfigManagerFactory.create(COMPONENT, cmdLine);
        try {
            assertEquals("INFO", cm.getFullConfig().getAsJsonObject("logging")
                    .get("level").getAsString());
            assertEquals(2, cm.getGlobalConfig().get("v").getAsInt());
        } finally {
            cm.close();
        }
    }

    @Test
    void inheritedStreamingDefinitionsRemainOrdinaryEffectiveConfig(@TempDir Path dir)
            throws Exception {
        write(dir, "shared.json", """
                {"streaming":{"streams":[{"name":"telemetry",
                 "sink":{"type":"kinesis","streamName":"site-dallas-telemetry"},
                 "buffer":{"type":"disk","path":"/tmp/{ComponentName}/telemetry"}}]}}""");
        Path component = write(dir, "config.json", """
                {"extends":"shared.json","component":{"global":{}}}""");

        ConfigManager cm = ConfigManagerFactory.create(COMPONENT, fileCmd(component, false));
        try {
            JsonArray streams = cm.getFullConfig().getAsJsonObject("streaming")
                    .getAsJsonArray("streams");
            assertEquals(1, streams.size());
            assertEquals("telemetry", streams.get(0).getAsJsonObject().get("name").getAsString());
        } finally {
            cm.close();
        }
    }

    @Test
    void invalidSharedConfigControlTypeFailsStartup(@TempDir Path dir) throws IOException {
        Path component = write(dir, "config.json", """
                {"sharedConfig":"false","component":{"global":{}}}""");
        assertThrows(ConfigurationException.class,
                () -> ConfigManagerFactory.create(COMPONENT, fileCmd(component, false)));
    }

    @Test
    void configComponentLegacyPushPreservesPreviousBaseLayer() throws Exception {
        ParsedCommandLine cmdLine = new ParsedCommandLine();
        cmdLine.configArgs = new String[]{"CONFIG_COMPONENT"};
        cmdLine.thingName = THING;
        FakeConfigComponentMessaging messaging = new FakeConfigComponentMessaging(parse("""
                {"base":{"logging":{"level":"INFO"},"tags":{"site":"dallas"}},
                 "component":{"component":{"global":{"v":1}}}}"""));

        ConfigManager cm = ConfigManagerFactory.create(COMPONENT, cmdLine, messaging);
        cm.completeInitialization();
        try {
            assertEquals("INFO", cm.getFullConfig().getAsJsonObject("logging")
                    .get("level").getAsString());

            messaging.push(parse("""
                    {"component":{"global":{"v":2}},"tags":{"component":"split"}}"""));

            JsonObject effective = cm.getFullConfig();
            assertEquals("INFO", effective.getAsJsonObject("logging")
                    .get("level").getAsString());
            assertEquals("dallas", effective.getAsJsonObject("tags")
                    .get("site").getAsString());
            assertEquals("split", effective.getAsJsonObject("tags")
                    .get("component").getAsString());
            assertEquals(2, cm.getGlobalConfig().get("v").getAsInt());
        } finally {
            cm.close();
        }
    }

    private static final class FakeConfigComponentMessaging extends MessagingClient {
        private final JsonObject replyBody;
        private BiConsumer<String, Message> callback;

        private FakeConfigComponentMessaging(JsonObject replyBody) {
            this.replyBody = replyBody;
        }

        @Override
        public void subscribe(String topicFilter, BiConsumer<String, Message> callback) {
            this.callback = callback;
        }

        @Override
        public ReplyFuture request(String topic, Message request) {
            ReplyFuture future = new ReplyFuture("reply-topic");
            future.complete(MessageBuilder.create("Config", "1.0")
                    .withPayload(replyBody)
                    .build());
            return future;
        }

        private void push(JsonObject payload) {
            if (callback == null) {
                throw new AssertionError("CONFIG_COMPONENT set-config subscription was not registered");
            }
            callback.accept("ecv1/gw-01/SplitComponent/main/cmd/set-config",
                    MessageBuilder.create("SetConfig", "1.0")
                            .withPayload(payload)
                            .build());
        }
    }
}
