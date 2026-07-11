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
import java.util.concurrent.atomic.AtomicInteger;
import java.util.function.BiConsumer;

import static org.junit.jupiter.api.Assertions.*;

class HierarchicalConfigTest {
    private static final String COMPONENT = "com.test.opcua-adapter";
    private static final String THING = "gw-01";
    private static final Path VECTOR_DIR = Path.of("..", "..",
            "hierarchical-config-test-vectors");

    private static JsonObject parse(String json) {
        return JsonParser.parseString(json).getAsJsonObject();
    }

    private static JsonObject readVector(String name) throws IOException {
        return JsonParser.parseString(Files.readString(VECTOR_DIR.resolve(name)))
                .getAsJsonObject();
    }

    private static Path write(Path dir, String name, String json) throws IOException {
        Path path = dir.resolve(name);
        Files.writeString(path, json, StandardCharsets.UTF_8);
        return path;
    }

    private static ParsedCommandLine configComponentCmdLine(String component) {
        ParsedCommandLine cmdLine = new ParsedCommandLine();
        cmdLine.configArgs = new String[]{"CONFIG_COMPONENT"};
        cmdLine.thingName = THING;
        return cmdLine;
    }

    private static ParsedCommandLine fileCmd(Path config) {
        ParsedCommandLine cmdLine = new ParsedCommandLine();
        cmdLine.configArgs = new String[]{"FILE", config.toString()};
        cmdLine.thingName = THING;
        return cmdLine;
    }

    @Test
    void mergeVectorsProduceExpectedEffectiveConfigs() throws IOException {
        JsonArray cases = readVector("merge.json").getAsJsonArray("cases");

        for (JsonElement element : cases) {
            JsonObject testCase = element.getAsJsonObject();
            String name = testCase.get("name").getAsString();
            JsonArray layerInputs = testCase.getAsJsonObject("input").getAsJsonArray("layers");
            List<JsonObject> layers = new ArrayList<>();
            for (JsonElement layerElement : layerInputs) {
                layers.add(layerElement.getAsJsonObject().getAsJsonObject("config"));
            }

            JsonObject actual = DeepMerge.merge(layers);
            JsonObject expected = testCase.getAsJsonObject("expected")
                    .getAsJsonObject("effective");
            assertEquals(expected, actual, name);
        }
    }

    @Test
    void lineageBundleVectorsProduceExpectedEffectiveConfigsOrErrors() throws Exception {
        JsonArray cases = readVector("lineage-bundles.json").getAsJsonArray("cases");

        for (JsonElement element : cases) {
            JsonObject testCase = element.getAsJsonObject();
            String name = testCase.get("name").getAsString();
            JsonObject input = testCase.getAsJsonObject("input");
            JsonObject body = input.getAsJsonObject("body");
            String requestComponent = input.get("requestComponent").getAsString();
            JsonObject expected = testCase.getAsJsonObject("expected");
            FakeConfigComponentMessaging messaging = new FakeConfigComponentMessaging(body);

            if (expected.has("error")) {
                ConfigurationException ex = assertThrows(ConfigurationException.class,
                        () -> ConfigManagerFactory.create("com.test." + requestComponent,
                                configComponentCmdLine(requestComponent), messaging),
                        name);
                assertContainsError(ex, expected.get("error").getAsString(), name);
                continue;
            }

            ConfigManager cm = ConfigManagerFactory.create("com.test." + requestComponent,
                    configComponentCmdLine(requestComponent), messaging);
            try {
                assertEquals(expected.getAsJsonObject("effective"), cm.getFullConfig(), name);
            } finally {
                cm.close();
            }
        }
    }

    @Test
    void errorVectorsRetainPreviousEffectiveSnapshotAndNotifyOnlyOnSuccess() throws Exception {
        JsonArray cases = readVector("errors.json").getAsJsonArray("cases");

        for (JsonElement element : cases) {
            JsonObject testCase = element.getAsJsonObject();
            String name = testCase.get("name").getAsString();
            JsonObject input = testCase.getAsJsonObject("input");
            JsonObject previous = input.getAsJsonObject("previousEffective");
            FakeConfigComponentMessaging messaging = new FakeConfigComponentMessaging(
                    lineageBundle(previous));
            ConfigManager cm = ConfigManagerFactory.create(COMPONENT,
                    configComponentCmdLine("opcua-adapter"), messaging);
            AtomicInteger notifications = new AtomicInteger();
            cm.addConfigChangeListener(() -> {
                notifications.incrementAndGet();
                return true;
            });
            cm.completeInitialization();
            try {
                JsonObject update = input.has("body")
                        ? input.getAsJsonObject("body")
                        : input.getAsJsonObject("push");
                messaging.push(update);

                JsonObject expected = testCase.getAsJsonObject("expected");
                JsonObject expectedEffective = expected.has("effective")
                        ? expected.getAsJsonObject("effective")
                        : previous;
                assertEquals(expectedEffective, cm.getFullConfig(), name);
                int expectedNotifications = expected.get("notifyListeners").getAsBoolean() ? 1 : 0;
                assertEquals(expectedNotifications, notifications.get(), name);
            } finally {
                cm.close();
            }
        }
    }

    @Test
    void directFileProviderDoesNotReadDefaultSharedConfig(@TempDir Path dir) throws Exception {
        write(dir, "shared.json", """
                {"component":{"global":{"shared":true}},"logging":{"level":"WARN"}}""");
        Path component = write(dir, "config.json", """
                {"component":{"global":{"v":1}}}""");

        ConfigManager cm = ConfigManagerFactory.create(COMPONENT, fileCmd(component));
        try {
            assertEquals(1, cm.getGlobalConfig().get("v").getAsInt());
            assertFalse(cm.getFullConfig().has("logging"));
        } finally {
            cm.close();
        }
    }

    private static JsonObject lineageBundle(JsonObject effectiveConfig) {
        JsonObject layer = new JsonObject();
        layer.addProperty("id", "component/opcua-adapter");
        layer.addProperty("kind", "component");
        layer.addProperty("component", "opcua-adapter");
        layer.add("config", effectiveConfig);
        JsonArray layers = new JsonArray();
        layers.add(layer);

        JsonObject bundle = new JsonObject();
        bundle.addProperty("lineageVersion", 1);
        bundle.addProperty("catalogVersion", "test");
        bundle.addProperty("component", "opcua-adapter");
        bundle.add("layers", layers);
        return bundle;
    }

    private static void assertContainsError(ConfigurationException ex, String code, String name) {
        Throwable cursor = ex;
        while (cursor != null) {
            if (cursor.getMessage() != null && cursor.getMessage().contains(code)) {
                return;
            }
            cursor = cursor.getCause();
        }
        fail(name + " expected error code " + code + " in " + ex);
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
        public void subscribeAcknowledged(String topicFilter,
                                           BiConsumer<String, Message> callback,
                                           int maxConcurrency,
                                           int maxMessages,
                                           java.time.Duration timeout) {
            if (timeout == null || timeout.isZero() || timeout.isNegative()) {
                throw new IllegalArgumentException("timeout must be positive");
            }
            this.callback = callback;
        }

        @Override
        public void unsubscribe(String topicFilter) {
            this.callback = null;
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
            callback.accept("ecv1/gw-01/opcua-adapter/main/cmd/set-config",
                    MessageBuilder.create("SetConfig", "1.0")
                            .withPayload(payload)
                            .build());
        }
    }
}
