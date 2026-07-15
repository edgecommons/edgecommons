import com.mbreissi.edgecommons.EdgeCommons;
import com.mbreissi.edgecommons.commands.CommandInbox;
import com.mbreissi.edgecommons.commands.CommandOutcome;
import com.mbreissi.edgecommons.heartbeat.InstanceConnectivity;
import com.mbreissi.edgecommons.logging.LogRecord;
import com.mbreissi.edgecommons.messaging.Message;
import com.mbreissi.edgecommons.messaging.MessageBuilder;
import com.mbreissi.edgecommons.messaging.MessageIdentity;
import com.mbreissi.edgecommons.messaging.MessagingClient;
import com.mbreissi.edgecommons.messaging.MessagingConfiguration;
import com.mbreissi.edgecommons.messaging.MessagingProvider;
import com.mbreissi.edgecommons.messaging.MessageTags;
import com.mbreissi.edgecommons.messaging.Qos;
import com.mbreissi.edgecommons.messaging.ReservedTopicException;
import com.mbreissi.edgecommons.messaging.proto.MessageBodyCase;
import com.mbreissi.edgecommons.messaging.providers.greengrass.GreengrassMessagingProvider;
import com.mbreissi.edgecommons.messaging.providers.standalone.StandaloneMessagingProvider;
import com.mbreissi.edgecommons.uns.Uns;
import com.mbreissi.edgecommons.uns.UnsClass;
import com.google.gson.Gson;
import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonNull;
import com.google.gson.JsonObject;
import com.google.gson.JsonPrimitive;

import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.Base64;
import java.util.HexFormat;
import java.util.Map;
import java.time.Duration;
import java.nio.channels.FileChannel;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;
import java.nio.charset.StandardCharsets;

/**
 * Cross-language interop node (Java) for edgecommons. Shared CLI contract:
 *   InteropNode responder &lt;request_topic&gt;
 *   InteropNode request   &lt;request_topic&gt; &lt;token&gt;
 *   InteropNode uns-pub   &lt;identityJson&gt; &lt;class&gt; [channel]
 *   InteropNode uns-sub   &lt;topic&gt;
 *   InteropNode uns-guard
 *   InteropNode status-responder     &lt;component&gt;
 *   InteropNode status-request       &lt;component&gt;
 *   InteropNode state-instances-pub  &lt;component&gt;
 *   InteropNode state-instances-sub  &lt;component&gt;
 * Local-only MQTT transport against localhost:1883. Messages are built WITHOUT a
 * config service — the envelope legally omits {@code identity} unless an explicit
 * identity is stamped (the UNS roles), and {@code tags.thing} no longer exists.
 */
public class InteropNode {
    static final String LANG = "java";
    static final Gson GSON = new Gson();

    static String host() {
        String h = System.getenv("EDGECOMMONS_IT_MQTT_HOST");
        return h != null ? h : "localhost";
    }

    static String port() {
        String p = System.getenv("EDGECOMMONS_IT_MQTT_PORT");
        return p != null ? p : "1883";
    }

    static StandaloneMessagingProvider provider(String suffix) {
        String json = "{ \"messaging\": { \"local\": {\"type\":\"mqtt\",\"host\":\"" + host()
                + "\",\"port\":" + port() + ",\"clientId\":\"interop-java-" + suffix + "-"
                + ProcessHandle.current().pid() + "\"} } }";
        MessagingConfiguration cfg = GSON.fromJson(json, MessagingConfiguration.class);
        return new StandaloneMessagingProvider(cfg, "interop-java");
    }

    static String logComponentToken() {
        return "interop-log-" + LANG;
    }

    static Path writeCommandRuntimeConfig(String componentToken) throws Exception {
        JsonObject cfg = new JsonObject();
        JsonObject component = new JsonObject();
        component.addProperty("token", componentToken);
        cfg.add("component", component);

        JsonObject local = new JsonObject();
        local.addProperty("type", "mqtt");
        local.addProperty("host", host());
        local.addProperty("port", Integer.parseInt(port()));
        local.addProperty("clientId", "interop-" + LANG + "-deferred-runtime-"
                + ProcessHandle.current().pid());
        JsonObject messaging = new JsonObject();
        messaging.add("local", local);
        messaging.addProperty("requestTimeoutSeconds", 4);
        cfg.add("messaging", messaging);

        JsonObject heartbeat = new JsonObject();
        heartbeat.addProperty("enabled", false);
        cfg.add("heartbeat", heartbeat);
        JsonObject health = new JsonObject();
        health.addProperty("enabled", false);
        cfg.add("health", health);

        Path path = Files.createTempFile("edgecommons-deferred-" + LANG + "-", ".json");
        Files.writeString(path, GSON.toJson(cfg), StandardCharsets.UTF_8);
        return path;
    }

    /**
     * The fixed interop device (IoT Thing) name every node's real-runtime roles run under, so a
     * requester/subscriber can derive a peer's UNS topics from the component token alone.
     */
    static final String INTEROP_DEVICE = "interop-device";

    /** The component's own command inbox topic for one verb (component scope, D-U28: no instance). */
    static String commandTopic(String component, String verb) {
        return "ecv1/" + INTEROP_DEVICE + "/" + component + "/cmd/" + verb;
    }

    /** The component's reserved {@code state} keepalive topic (component scope, D-U28: no instance). */
    static String stateTopic(String component) {
        return "ecv1/" + INTEROP_DEVICE + "/" + component + "/state";
    }

    /**
     * The canonical per-instance connectivity sample every language's interop node reports through
     * the ONE component-supplied provider that feeds both surfaces — the {@code state} keepalive's
     * {@code instances[]} (push) and the built-in {@code status} verb (pull). It pins the contract:
     * cam-01 carries every optional member (with an open {@code attributes} bag holding an array, a
     * string and a number); cam-02 is disconnected with a richer {@code state} token; cam-03 is the
     * minimal element, whose optional members must be OMITTED — never emitted as null/empty.
     */
    static java.util.List<InstanceConnectivity> canonicalInstances() {
        JsonArray capabilities = new JsonArray();
        capabilities.add("ptz");
        capabilities.add("snapshot");
        Map<String, JsonElement> attributes = new java.util.LinkedHashMap<>();
        attributes.put("capabilities", capabilities);
        attributes.put("vendor", new JsonPrimitive("acme"));
        attributes.put("retries", new JsonPrimitive(0));
        return java.util.List.of(
                InstanceConnectivity.of("cam-01", true, "rtsp://cam-01/stream")
                        .withState("ONLINE")
                        .withAttributes(attributes),
                InstanceConnectivity.of("cam-02", false, "connect timed out")
                        .withState("BACKOFF"),
                InstanceConnectivity.of("cam-03", true));
    }

    /**
     * The runtime config for the connectivity roles: a real component on the local broker, with the
     * heartbeat (the {@code state} keepalive that PUSHES {@code instances[]}) enabled only for the
     * publisher role. The command inbox — which serves the {@code status} PULL — is always on.
     */
    static Path writeConnectivityRuntimeConfig(String componentToken, boolean heartbeatEnabled)
            throws Exception {
        JsonObject cfg = new JsonObject();
        JsonObject component = new JsonObject();
        component.addProperty("token", componentToken);
        cfg.add("component", component);

        JsonObject local = new JsonObject();
        local.addProperty("type", "mqtt");
        local.addProperty("host", host());
        local.addProperty("port", Integer.parseInt(port()));
        local.addProperty("clientId", "interop-" + LANG + "-connectivity-"
                + ProcessHandle.current().pid());
        JsonObject messaging = new JsonObject();
        messaging.add("local", local);
        messaging.addProperty("requestTimeoutSeconds", 10);
        cfg.add("messaging", messaging);

        JsonObject heartbeat = new JsonObject();
        heartbeat.addProperty("enabled", heartbeatEnabled);
        heartbeat.addProperty("intervalSecs", 2);
        heartbeat.addProperty("destination", "local");
        cfg.add("heartbeat", heartbeat);
        JsonObject health = new JsonObject();
        health.addProperty("enabled", false);
        cfg.add("health", health);

        Path path = Files.createTempFile("edgecommons-connectivity-" + LANG + "-", ".json");
        Files.writeString(path, GSON.toJson(cfg), StandardCharsets.UTF_8);
        return path;
    }

    /**
     * Write and flush the bounded P1 durable-acceptance marker before exposing a deferred reply.
     * The marker is intentionally not an in-memory JSON flag: a successful terminal reply may
     * claim {@code durablyAccepted} only after this local persistence boundary completes.
     */
    static Path writeDurableAcceptanceMarker() throws java.io.IOException {
        Path marker = Files.createTempFile("edgecommons-p1-accept-" + LANG + "-", ".marker");
        try (FileChannel channel = FileChannel.open(marker,
                StandardOpenOption.WRITE, StandardOpenOption.TRUNCATE_EXISTING)) {
            var content = StandardCharsets.UTF_8.encode("accepted\n");
            while (content.hasRemaining()) {
                channel.write(content);
            }
            channel.force(true);
            return marker;
        } catch (java.io.IOException error) {
            Files.deleteIfExists(marker);
            throw error;
        }
    }

    static void removeDurableAcceptanceMarker(Path marker) {
        try {
            Files.deleteIfExists(marker);
        } catch (java.io.IOException ignored) {
            // The marker is a test-harness artifact; cleanup is best effort after settlement.
        }
    }

    static Path writeLogRuntimeConfig() throws Exception {
        JsonObject cfg = new JsonObject();

        JsonObject component = new JsonObject();
        component.addProperty("token", logComponentToken());
        cfg.add("component", component);

        JsonObject local = new JsonObject();
        local.addProperty("type", "mqtt");
        local.addProperty("host", host());
        local.addProperty("port", Integer.parseInt(port()));
        local.addProperty("clientId", "interop-" + LANG + "-log-runtime-" + ProcessHandle.current().pid());
        JsonObject messaging = new JsonObject();
        messaging.add("local", local);
        messaging.addProperty("requestTimeoutSeconds", 2);
        cfg.add("messaging", messaging);

        JsonObject heartbeat = new JsonObject();
        heartbeat.addProperty("enabled", false);
        cfg.add("heartbeat", heartbeat);
        JsonObject health = new JsonObject();
        health.addProperty("enabled", false);
        cfg.add("health", health);

        JsonObject publish = new JsonObject();
        publish.addProperty("enabled", true);
        publish.addProperty("destination", "local");
        publish.addProperty("minLevel", "TRACE");
        publish.addProperty("captureNative", false);
        publish.addProperty("captureConsole", false);
        JsonObject redaction = new JsonObject();
        redaction.addProperty("enabled", false);
        publish.add("redaction", redaction);
        JsonObject logging = new JsonObject();
        logging.addProperty("level", "WARN");
        logging.add("publish", publish);
        cfg.add("logging", logging);

        Path path = Files.createTempFile("edgecommons-log-" + LANG + "-", ".json");
        Files.writeString(path, GSON.toJson(cfg), StandardCharsets.UTF_8);
        return path;
    }

    static String[] logRuntimeArgs(Path path) {
        String p = path.toString();
        return new String[] {
                "--platform", "HOST",
                "--transport", "MQTT", p,
                "-c", "FILE", p,
                "-t", "interop-device"
        };
    }

    static String wireIdentityDevice(JsonObject identity) {
        if (identity == null || !identity.has("hier") || !identity.get("hier").isJsonArray()) {
            return null;
        }
        var hier = identity.getAsJsonArray("hier");
        if (hier.isEmpty()) {
            return null;
        }
        JsonObject tail = hier.get(hier.size() - 1).getAsJsonObject();
        return tail.has("value") ? tail.get("value").getAsString() : null;
    }

    static JsonElement asElement(Object o) {
        if (o instanceof JsonElement je) {
            return je;
        }
        return GSON.toJsonTree(o);
    }

    static String ggTopic(String runId, String publisher, String subscriber) {
        return "edgecommons/interop/binary/" + runId + "/" + publisher + "/" + subscriber;
    }

    static String ggTypedTopic(String runId, String publisher, String subscriber) {
        return "edgecommons/interop/typed/" + runId + "/" + publisher + "/" + subscriber;
    }

    static JsonObject typedBody(byte[] bytes) {
        JsonObject body = new JsonObject();
        JsonObject signal = new JsonObject();
        signal.addProperty("id", "camera-1/roi-17/thumbnail");
        signal.addProperty("name", "Thumbnail");
        body.add("signal", signal);
        com.google.gson.JsonArray samples = new com.google.gson.JsonArray();
        JsonObject sample = new JsonObject();
        sample.add("value", Message.binaryBodyMarker(bytes));
        sample.addProperty("quality", "GOOD");
        sample.addProperty("sourceTsMs", 1783360799900L);
        sample.addProperty("serverTsMs", 1783360800000L);
        samples.add(sample);
        body.add("samples", samples);
        return body;
    }

    static String publisherFromGgTopic(String topic) {
        String[] parts = topic.split("/");
        return parts.length >= 2 ? parts[parts.length - 2] : null;
    }

    static Path ggReadyPath(String runId, String lang) {
        return Path.of("/tmp", "edgecommons_gg_ipc_binary_ready_" + lang + "_" + runId);
    }

    static Path ggLogReadyPath(String runId, String lang) {
        return Path.of("/tmp", "edgecommons_gg_ipc_log_ready_" + lang + "_" + runId);
    }

    static java.util.List<String> waitForGgReady(String runId, String[] expectedLangs)
            throws InterruptedException {
        long readyWaitMs = (long) (Double.parseDouble(
                System.getenv().getOrDefault("EDGECOMMONS_GG_READY_WAIT_SECS", "180")) * 1000);
        long deadline = System.currentTimeMillis() + readyWaitMs;
        while (System.currentTimeMillis() < deadline) {
            java.util.ArrayList<String> missing = new java.util.ArrayList<>();
            for (String lang : expectedLangs) {
                if (!Files.exists(ggReadyPath(runId, lang))) {
                    missing.add(lang);
                }
            }
            if (missing.isEmpty()) {
                return missing;
            }
            Thread.sleep(200);
        }
        java.util.ArrayList<String> missing = new java.util.ArrayList<>();
        for (String lang : expectedLangs) {
            if (!Files.exists(ggReadyPath(runId, lang))) {
                missing.add(lang);
            }
        }
        return missing;
    }

    static java.util.List<String> waitForGgLogReady(String runId, String[] expectedLangs)
            throws InterruptedException {
        long readyWaitMs = (long) (Double.parseDouble(
                System.getenv().getOrDefault("EDGECOMMONS_GG_READY_WAIT_SECS", "180")) * 1000);
        long deadline = System.currentTimeMillis() + readyWaitMs;
        while (System.currentTimeMillis() < deadline) {
            java.util.ArrayList<String> missing = new java.util.ArrayList<>();
            for (String lang : expectedLangs) {
                if (!Files.exists(ggLogReadyPath(runId, lang))) {
                    missing.add(lang);
                }
            }
            if (missing.isEmpty()) {
                return missing;
            }
            Thread.sleep(200);
        }
        java.util.ArrayList<String> missing = new java.util.ArrayList<>();
        for (String lang : expectedLangs) {
            if (!Files.exists(ggLogReadyPath(runId, lang))) {
                missing.add(lang);
            }
        }
        return missing;
    }

    static String[] ggLogRuntimeArgs(Path path) {
        return new String[] {
                "--platform", "GREENGRASS",
                "--transport", "IPC",
                "-c", "FILE", path.toString(),
                "-t", "interop-device"
        };
    }

    static Path ggP1ReadyPath(String runId, String actor) {
        return Path.of("/tmp", "edgecommons_gg_ipc_p1_ready_" + actor + "_" + runId);
    }

    static java.util.List<String> waitForGgP1Ready(String runId, String[] expectedActors)
            throws InterruptedException {
        long readyWaitMs = (long) (Double.parseDouble(
                System.getenv().getOrDefault("EDGECOMMONS_GG_READY_WAIT_SECS", "180")) * 1000);
        long deadline = System.currentTimeMillis() + readyWaitMs;
        while (System.currentTimeMillis() < deadline) {
            java.util.ArrayList<String> missing = new java.util.ArrayList<>();
            for (String actor : expectedActors) {
                if (!Files.exists(ggP1ReadyPath(runId, actor))) {
                    missing.add(actor);
                }
            }
            if (missing.isEmpty()) {
                return missing;
            }
            Thread.sleep(200);
        }
        java.util.ArrayList<String> missing = new java.util.ArrayList<>();
        for (String actor : expectedActors) {
            if (!Files.exists(ggP1ReadyPath(runId, actor))) {
                missing.add(actor);
            }
        }
        return missing;
    }

    static String ggP1TargetActor(String targetLanguage, String senderActor) {
        return targetLanguage.equals("rust") && senderActor.equals("rust")
                ? "rustpeer" : targetLanguage;
    }

    static String ggP1CommandTopic(String actor) {
        return "ecv1/interop-device/interop-p1-" + actor + "/cmd/deferred";
    }

    static String ggP1ConfirmedTopic(String runId, String publisher, String targetActor) {
        return "edgecommons/interop/p1/" + runId + "/confirmed/" + publisher + "/" + targetActor;
    }

    static JsonObject sendGgP1Deferred(
            MessagingProvider provider, String runId, String senderActor,
            String targetLanguage, String targetActor) throws Exception {
        String token = runId + ":" + senderActor + "->" + targetLanguage;
        String replyTopic = "edgecommons/interop/p1/" + runId + "/reply/" + senderActor
                + "/" + targetActor + "/" + java.util.UUID.randomUUID();
        java.util.List<Message> replies = java.util.Collections.synchronizedList(
                new java.util.ArrayList<>());
        CountDownLatch firstReply = new CountDownLatch(1);
        provider.subscribe(replyTopic, (topic, reply) -> {
            replies.add(reply);
            firstReply.countDown();
        }, 1, 2);
        JsonObject requestBody = new JsonObject();
        requestBody.addProperty("token", token);
        requestBody.addProperty("from", LANG);
        requestBody.addProperty("actor", senderActor);
        Message request = MessageBuilder.create("deferred", "1.0")
                .withCommand(requestBody).withReplyTo(replyTopic).build();
        String correlation = request.getCorrelationId();
        provider.publish(ggP1CommandTopic(targetActor), request);
        JsonObject out = new JsonObject();
        out.addProperty("target_actor", targetActor);
        out.addProperty("expected_token", token);
        out.addProperty("expected_responder", targetLanguage);
        out.addProperty("expected_responder_actor", targetActor);
        if (!firstReply.await(8, TimeUnit.SECONDS)) {
            out.addProperty("ok", false);
            out.addProperty("error", "timeout");
            return out;
        }
        Thread.sleep(750);
        Message reply;
        int replyCount;
        synchronized (replies) {
            replyCount = replies.size();
            reply = replies.get(0);
        }
        JsonElement replyBody = asElement(reply.getBody());
        JsonObject result = replyBody.isJsonObject() && replyBody.getAsJsonObject().has("result")
                ? replyBody.getAsJsonObject().getAsJsonObject("result") : null;
        boolean correlationMatch = correlation != null && correlation.equals(reply.getCorrelationId());
        boolean ok = replyCount == 1 && correlationMatch && replyBody.isJsonObject()
                && replyBody.getAsJsonObject().has("ok")
                && replyBody.getAsJsonObject().get("ok").getAsBoolean()
                && result != null && token.equals(result.get("token").getAsString())
                && result.get("durablyAccepted").getAsBoolean()
                && targetLanguage.equals(result.get("responder").getAsString())
                && targetActor.equals(result.get("responderActor").getAsString());
        out.addProperty("ok", ok);
        out.addProperty("reply_count", replyCount);
        out.addProperty("correlation_match", correlationMatch);
        out.addProperty("duplicate_window_ms", 750);
        out.add("reply_body", replyBody);
        return out;
    }

    static void runGgP1Matrix(String[] args) throws Exception {
        String runId = args[1];
        String[] languages = args[2].split(",");
        String[] expectedActors = System.getenv().getOrDefault(
                "EDGECOMMONS_GG_READY_LANGS", args[2]).split(",");
        String actor = System.getenv().getOrDefault("EDGECOMMONS_GG_READY_LANG", LANG);
        boolean canonicalActor = !actor.equals("rustpeer");
        long subscribeDelayMs = (long) (Double.parseDouble(
                System.getenv().getOrDefault("EDGECOMMONS_GG_SUBSCRIBE_DELAY_SECS", "2")) * 1000);
        long waitMs = (long) (Double.parseDouble(
                System.getenv().getOrDefault("EDGECOMMONS_GG_WAIT_SECS", "90")) * 1000);
        java.util.List<String> expectedPublishers = new java.util.ArrayList<>();
        if (actor.equals("rust")) {
            for (String language : languages) {
                if (!language.equals("rust")) expectedPublishers.add(language);
            }
        } else if (canonicalActor) {
            java.util.Collections.addAll(expectedPublishers, languages);
        } else {
            expectedPublishers.add("rust");
        }

        MessagingProvider provider = new GreengrassMessagingProvider(true);
        java.util.concurrent.ConcurrentHashMap<String, java.util.List<JsonObject>> received =
                new java.util.concurrent.ConcurrentHashMap<>();
        java.util.concurrent.ConcurrentHashMap<String, String> errors =
                new java.util.concurrent.ConcurrentHashMap<>();
        CountDownLatch firstConfirmed = new CountDownLatch(1);
        EdgeCommons gg = null;
        Path path = null;
        try {
            path = writeCommandRuntimeConfig("interop-p1-" + actor);
            gg = new EdgeCommons("com.mbreissi.edgecommons.interop." + LANG + ".P1Responder",
                    ggLogRuntimeArgs(path));
            CommandInbox inbox = gg.getCommands();
            if (inbox == null) throw new IllegalStateException("runtime did not expose command inbox");
            String responderActor = actor;
            inbox.registerOutcome("deferred", request -> {
                CommandInbox.DeferredReply deferred = inbox.defer(request, Duration.ofSeconds(4));
                JsonObject requestBody = asElement(request.getBody()).getAsJsonObject();
                Path acceptanceMarker;
                try {
                    acceptanceMarker = writeDurableAcceptanceMarker();
                } catch (java.io.IOException error) {
                    deferred.discard();
                    return CommandOutcome.error("ACCEPTANCE_FAILED", "work was not accepted");
                }
                String token = requestBody.get("token").getAsString();
                if (!deferred.activate()) {
                    removeDurableAcceptanceMarker(acceptanceMarker);
                    return CommandOutcome.error("ACTIVATION_FAILED", "deferred token was not open");
                }
                return CommandOutcome.deferredWithContinuation(deferred, () -> {
                    try {
                        JsonObject result = new JsonObject();
                        result.addProperty("token", token);
                        result.addProperty("responder", LANG);
                        result.addProperty("responderActor", responderActor);
                        result.addProperty("durablyAccepted", true);
                        deferred.settleSuccess(result);
                    } finally {
                        removeDurableAcceptanceMarker(acceptanceMarker);
                    }
                });
            });
            provider.subscribe("edgecommons/interop/p1/" + runId + "/confirmed/+/" + actor,
                    (topic, message) -> {
                        String publisher = publisherFromGgTopic(topic);
                        try {
                            JsonObject body = asElement(message.getBody()).getAsJsonObject();
                            boolean valid = publisher != null
                                    && runId.equals(body.get("runId").getAsString())
                                    && publisher.equals(body.get("publisher").getAsString())
                                    && actor.equals(body.get("targetActor").getAsString())
                                    && body.get("strict").getAsBoolean();
                            JsonObject item = new JsonObject();
                            item.addProperty("ok", valid);
                            item.addProperty("topic", topic);
                            item.add("body", body);
                            received.computeIfAbsent(publisher == null ? "unknown" : publisher,
                                    unused -> java.util.Collections.synchronizedList(new java.util.ArrayList<>()))
                                    .add(item);
                        } catch (Exception error) {
                            errors.put("confirmed:" + (publisher == null ? "unknown" : publisher),
                                    error.toString());
                        } finally {
                            firstConfirmed.countDown();
                        }
                    }, 1, 32);
            System.out.println("READY");
            System.out.flush();
            Files.writeString(ggP1ReadyPath(runId, actor), Long.toString(System.currentTimeMillis()),
                    StandardCharsets.UTF_8);

            java.util.List<String> readyMissing = waitForGgP1Ready(runId, expectedActors);
            JsonObject deferredRequests = new JsonObject();
            JsonObject confirmedPublishes = new JsonObject();
            if (readyMissing.isEmpty() && canonicalActor) {
                Thread.sleep(subscribeDelayMs);
                for (String targetLanguage : languages) {
                    String targetActor = ggP1TargetActor(targetLanguage, actor);
                    try {
                        deferredRequests.add(targetLanguage,
                                sendGgP1Deferred(provider, runId, actor, targetLanguage, targetActor));
                    } catch (Exception error) {
                        JsonObject failure = new JsonObject();
                        failure.addProperty("ok", false);
                        failure.addProperty("target_actor", targetActor);
                        failure.addProperty("error", error.getClass().getSimpleName());
                        deferredRequests.add(targetLanguage, failure);
                    }
                    JsonObject body = new JsonObject();
                    body.addProperty("runId", runId);
                    body.addProperty("publisher", LANG);
                    body.addProperty("publisherActor", actor);
                    body.addProperty("targetLanguage", targetLanguage);
                    body.addProperty("targetActor", targetActor);
                    body.addProperty("strict", true);
                    JsonObject published = new JsonObject();
                    published.addProperty("target_actor", targetActor);
                    try {
                        Message message = MessageBuilder.create("InteropConfirmed", "1.0")
                                .withPayload(body).build();
                        provider.publishConfirmed(ggP1ConfirmedTopic(runId, LANG, targetActor),
                                message.toBytes(), Qos.AT_LEAST_ONCE, Duration.ofSeconds(5));
                        published.addProperty("ok", true);
                        published.addProperty("confirmed", true);
                        published.addProperty("qos", 1);
                    } catch (Exception error) {
                        published.addProperty("ok", false);
                        published.addProperty("error", error.getClass().getSimpleName());
                    }
                    confirmedPublishes.add(targetLanguage, published);
                }
            }
            firstConfirmed.await(waitMs, TimeUnit.MILLISECONDS);
            long deadline = System.currentTimeMillis() + waitMs;
            while (System.currentTimeMillis() < deadline && expectedPublishers.stream()
                    .anyMatch(publisher -> !received.containsKey(publisher))) {
                Thread.sleep(50);
            }
            Thread.sleep(750);

            JsonObject receivedJson = new JsonObject();
            java.util.List<String> confirmedMissing = new java.util.ArrayList<>();
            boolean receiveOk = true;
            for (String publisher : expectedPublishers) {
                java.util.List<JsonObject> items = received.get(publisher);
                if (items == null) {
                    confirmedMissing.add(publisher);
                    receiveOk = false;
                    continue;
                }
                JsonObject evidence = new JsonObject();
                synchronized (items) {
                    evidence.addProperty("count", items.size());
                    evidence.add("items", GSON.toJsonTree(items));
                    evidence.addProperty("ok", items.size() == 1 && items.get(0).get("ok").getAsBoolean());
                }
                receiveOk = receiveOk && evidence.get("ok").getAsBoolean();
                receivedJson.add(publisher, evidence);
            }
            for (Map.Entry<String, java.util.List<JsonObject>> entry : received.entrySet()) {
                if (!receivedJson.has(entry.getKey())) {
                    JsonObject evidence = new JsonObject();
                    synchronized (entry.getValue()) {
                        evidence.addProperty("count", entry.getValue().size());
                        evidence.add("items", GSON.toJsonTree(entry.getValue()));
                        evidence.addProperty("ok", false);
                    }
                    receivedJson.add(entry.getKey(), evidence);
                    receiveOk = false;
                }
            }
            boolean requestsOk = !canonicalActor || deferredRequests.size() == languages.length;
            boolean publishesOk = !canonicalActor || confirmedPublishes.size() == languages.length;
            if (canonicalActor) {
                for (String language : languages) {
                    requestsOk = requestsOk && deferredRequests.getAsJsonObject(language).get("ok").getAsBoolean();
                    publishesOk = publishesOk && confirmedPublishes.getAsJsonObject(language).get("ok").getAsBoolean();
                }
            }
            boolean ok = readyMissing.isEmpty() && errors.isEmpty() && requestsOk && publishesOk && receiveOk;
            JsonObject result = new JsonObject();
            result.addProperty("schema", "edgecommons.gg-ipc-p1.v1");
            result.addProperty("ok", ok);
            result.addProperty("run_id", runId);
            result.addProperty("actor", actor);
            result.addProperty("language", LANG);
            result.addProperty("canonical_actor", canonicalActor);
            result.add("ready_missing", GSON.toJsonTree(readyMissing));
            result.add("deferred_requests", deferredRequests);
            result.add("confirmed_publishes", confirmedPublishes);
            result.add("confirmed_received", receivedJson);
            result.add("confirmed_missing", GSON.toJsonTree(confirmedMissing));
            result.add("errors", GSON.toJsonTree(errors));
            Files.writeString(Path.of("/tmp", "edgecommons_gg_ipc_p1_" + actor + "_" + runId + ".json"),
                    GSON.toJson(result), StandardCharsets.UTF_8);
            System.out.println(result);
            System.exit(ok ? 0 : 1);
        } finally {
            if (gg != null) gg.shutdown();
            if (path != null) Files.deleteIfExists(path);
            provider.close();
        }
    }

    /**
     * Canonical cross-language payload permutations, built as a plain {@link java.util.Map} (NOT a
     * JsonObject) so the Java sender exercises issue #13's {@code withPayload(Map)} -> Gson
     * serialization across the wire. {@code null} is tested inside an array (Gson drops null-valued
     * MAP entries — a documented divergence), so there is no top-level null key.
     */
    static java.util.Map<String, Object> typesMap() {
        java.util.Map<String, Object> m = new java.util.LinkedHashMap<>();
        m.put("b", true);
        m.put("bf", false);
        m.put("i", 42);
        m.put("ni", -7);
        m.put("fl", 3.5);
        m.put("slash", "a/b");
        m.put("quote", "x\"y");
        m.put("arr", java.util.Arrays.asList(1, "two", false, null));
        m.put("nullv", null);
        java.util.Map<String, Object> inner = new java.util.LinkedHashMap<>();
        inner.put("d", 2);
        m.put("nested", java.util.Collections.singletonMap("k", java.util.Arrays.asList(1, inner)));
        m.put("ea", java.util.Collections.emptyList());
        m.put("eo", java.util.Collections.emptyMap());
        return m;
    }

    public static void main(String[] args) throws Exception {
        String role = args[0];

        if (role.equals("responder")) {
            String topic = args[1];
            StandaloneMessagingProvider prov = provider("resp");
            prov.subscribe(topic, (t, request) -> {
                JsonObject body = new JsonObject();
                body.add("echo", asElement(request.getBody()));
                body.addProperty("responder", LANG);
                Message reply = MessageBuilder.create("InteropReply", "1.0")
                        .withPayload(body).build();
                prov.reply(request, reply);
            }, 1);
            System.out.println("READY");
            System.out.flush();
            Thread.sleep(Long.MAX_VALUE);
        } else if (role.equals("request")) {
            String topic = args[1];
            String token = args[2];
            StandaloneMessagingProvider prov = provider("req");
            // Issue #13: build the request body as a plain Map (NOT a JsonObject) so the Java sender
            // exercises the withPayload(Map) -> Gson serialization path end-to-end across the wire.
            java.util.Map<String, Object> reqBody = new java.util.LinkedHashMap<>();
            reqBody.put("token", token);
            reqBody.put("from", LANG);
            reqBody.put("types", typesMap());
            Message req = MessageBuilder.create("InteropRequest", "1.0")
                    .withPayload(reqBody).build();
            String corr = req.getCorrelationId();
            JsonObject out = new JsonObject();
            try {
                Message reply = prov.request(topic, req).get(8, TimeUnit.SECONDS);
                boolean match = corr != null && corr.equals(reply.getCorrelationId());
                JsonElement rbody = asElement(reply.getBody());
                out.addProperty("ok", true);
                out.addProperty("correlation_match", match);
                out.add("reply_body", rbody);
                System.out.println(out);
                boolean ok = match && rbody.isJsonObject()
                        && rbody.getAsJsonObject().has("responder")
                        && rbody.getAsJsonObject().getAsJsonObject("echo")
                                .get("token").getAsString().equals(token);
                prov.close();
                System.exit(ok ? 0 : 1);
            } catch (Exception e) {
                out.addProperty("ok", false);
                out.addProperty("error", e.getClass().getSimpleName());
                System.out.println(out);
                System.exit(1);
            }
        } else if (role.equals("deferred-responder")) {
            String componentToken = args[1];
            Path path = writeCommandRuntimeConfig(componentToken);
            EdgeCommons gg = null;
            try {
                gg = new EdgeCommons(
                        "com.mbreissi.edgecommons.interop." + LANG + ".DeferredResponder",
                        logRuntimeArgs(path));
                CommandInbox inbox = gg.getCommands();
                if (inbox == null) {
                    throw new IllegalStateException("runtime did not expose command inbox");
                }
                inbox.registerOutcome("deferred", request -> {
                    CommandInbox.DeferredReply deferred = inbox.defer(request, Duration.ofSeconds(4));
                    Path acceptanceMarker;
                    try {
                        acceptanceMarker = writeDurableAcceptanceMarker();
                    } catch (java.io.IOException error) {
                        deferred.discard();
                        return CommandOutcome.error("ACCEPTANCE_FAILED", "work was not accepted");
                    }
                    String acceptedToken = asElement(request.getBody())
                            .getAsJsonObject().get("token").getAsString();
                    if (!deferred.activate()) {
                        removeDurableAcceptanceMarker(acceptanceMarker);
                        return CommandOutcome.error("ACTIVATION_FAILED", "deferred token was not open");
                    }
                    return CommandOutcome.deferredWithContinuation(deferred, () -> {
                        try {
                            JsonObject result = new JsonObject();
                            result.addProperty("token", acceptedToken);
                            result.addProperty("responder", LANG);
                            result.addProperty("durablyAccepted", true);
                            deferred.settleSuccess(result);
                        } finally {
                            removeDurableAcceptanceMarker(acceptanceMarker);
                        }
                    });
                });
                System.out.println("READY");
                System.out.flush();
                Thread.sleep(Long.MAX_VALUE);
            } finally {
                if (gg != null) {
                    gg.shutdown();
                }
                Files.deleteIfExists(path);
            }
        } else if (role.equals("deferred-request")) {
            String topic = args[1];
            String token = args[2];
            StandaloneMessagingProvider prov = provider("deferredreq");
            String replyTopic = "interop/deferred/reply/" + LANG + "/"
                    + java.util.UUID.randomUUID();
            java.util.List<Message> replies = java.util.Collections.synchronizedList(
                    new java.util.ArrayList<>());
            CountDownLatch firstReply = new CountDownLatch(1);
            try {
                prov.subscribe(replyTopic, (t, reply) -> {
                    replies.add(reply);
                    firstReply.countDown();
                }, 1);
                JsonObject body = new JsonObject();
                body.addProperty("token", token);
                body.addProperty("from", LANG);
                Message request = MessageBuilder.create("deferred", "1.0")
                        .withCommand(body).withReplyTo(replyTopic).build();
                String correlation = request.getCorrelationId();
                prov.publish(topic, request);
                JsonObject out = new JsonObject();
                if (!firstReply.await(8, TimeUnit.SECONDS)) {
                    out.addProperty("ok", false);
                    out.addProperty("error", "timeout");
                    System.out.println(out);
                    System.exit(1);
                }
                // Retain the reply subscription long enough to expose a duplicate settlement.
                Thread.sleep(750);
                Message reply;
                int replyCount;
                synchronized (replies) {
                    replyCount = replies.size();
                    reply = replies.get(0);
                }
                boolean correlationMatch = correlation != null
                        && correlation.equals(reply.getCorrelationId());
                JsonElement replyBody = asElement(reply.getBody());
                JsonObject result = replyBody.isJsonObject()
                        && replyBody.getAsJsonObject().has("result")
                        ? replyBody.getAsJsonObject().getAsJsonObject("result") : null;
                boolean ok = replyCount == 1 && correlationMatch && replyBody.isJsonObject()
                        && replyBody.getAsJsonObject().has("ok")
                        && replyBody.getAsJsonObject().get("ok").getAsBoolean()
                        && result != null && token.equals(result.get("token").getAsString())
                        && result.get("durablyAccepted").getAsBoolean()
                        && result.has("responder");
                out.addProperty("ok", ok);
                out.addProperty("reply_count", replyCount);
                out.addProperty("correlation_match", correlationMatch);
                out.add("reply_body", replyBody);
                System.out.println(out);
                System.exit(ok ? 0 : 1);
            } finally {
                prov.close();
            }
        } else if (role.equals("confirmed-sub")) {
            String topic = args[1];
            String token = args[2];
            StandaloneMessagingProvider prov = provider("confirmedsub");
            java.util.List<Message> messages = java.util.Collections.synchronizedList(
                    new java.util.ArrayList<>());
            CountDownLatch firstMessage = new CountDownLatch(1);
            try {
                prov.subscribe(topic, (t, message) -> {
                    messages.add(message);
                    firstMessage.countDown();
                }, 1);
                System.out.println("READY");
                System.out.flush();
                JsonObject out = new JsonObject();
                if (!firstMessage.await(8, TimeUnit.SECONDS)) {
                    out.addProperty("ok", false);
                    out.addProperty("error", "timeout");
                    System.out.println(out);
                    System.exit(1);
                }
                Thread.sleep(750);
                Message message;
                int messageCount;
                synchronized (messages) {
                    messageCount = messages.size();
                    message = messages.get(0);
                }
                JsonElement body = asElement(message.getBody());
                boolean ok = messageCount == 1 && body.isJsonObject()
                        && token.equals(body.getAsJsonObject().get("token").getAsString())
                        && body.getAsJsonObject().has("from");
                out.addProperty("ok", ok);
                out.addProperty("message_count", messageCount);
                out.add("body", body);
                System.out.println(out);
                System.exit(ok ? 0 : 1);
            } finally {
                prov.close();
            }
        } else if (role.equals("confirmed-pub")) {
            String topic = args[1];
            String token = args[2];
            StandaloneMessagingProvider prov = provider("confirmedpub");
            JsonObject out = new JsonObject();
            try {
                JsonObject body = new JsonObject();
                body.addProperty("token", token);
                body.addProperty("from", LANG);
                Message message = MessageBuilder.create("InteropConfirmed", "1.0")
                        .withPayload(body).build();
                // This method returns only after the standalone MQTT client has received PUBACK.
                prov.publishConfirmed(topic, message.toBytes(), Qos.AT_LEAST_ONCE,
                        Duration.ofSeconds(5));
                out.addProperty("ok", true);
                out.addProperty("confirmed", true);
                out.addProperty("qos", 1);
                System.out.println(out);
            } catch (Exception e) {
                out.addProperty("ok", false);
                out.addProperty("error", e.getClass().getSimpleName());
                System.out.println(out);
                System.exit(1);
            } finally {
                prov.close();
            }
        } else if (role.equals("raw-sub")) {
            String topic = args[1];
            String token = args[2];
            StandaloneMessagingProvider prov = provider("rawsub");
            final CountDownLatch latch = new CountDownLatch(1);
            final Message[] box = new Message[1];
            prov.subscribe(topic, (t, m) -> {
                box[0] = m;
                latch.countDown();
            }, 1);
            System.out.println("READY");
            System.out.flush();
            JsonObject out = new JsonObject();
            if (!latch.await(10, TimeUnit.SECONDS)) {
                out.addProperty("ok", true);
                out.addProperty("delivered", false);
                out.addProperty("error", "timeout");
                System.out.println(out);
                System.exit(0);
            }
            out.addProperty("ok", false);
            out.addProperty("delivered", true);
            out.add("raw", asElement(box[0].getRaw()));
            out.add("body", asElement(box[0].getBody()));
            out.addProperty("expected_token", token);
            System.out.println(out);
            prov.close();
            System.exit(1);
        } else if (role.equals("raw-pub")) {
            String topic = args[1];
            String token = args[2];
            StandaloneMessagingProvider prov = provider("rawpub");
            JsonObject payload = new JsonObject();
            payload.addProperty("token", token);
            payload.addProperty("from", LANG);
            prov.publishRaw(topic, payload);
            Thread.sleep(500);
            prov.close();
        } else if (role.equals("binary-sub")) {
            String topic = args[1];
            String expectedHex = args[2].toLowerCase(java.util.Locale.ROOT);
            StandaloneMessagingProvider prov = provider("binsub");
            final CountDownLatch latch = new CountDownLatch(1);
            final Message[] box = new Message[1];
            final String[] error = new String[1];
            prov.subscribe(topic, (t, m) -> {
                try {
                    box[0] = m;
                } catch (Exception e) {
                    error[0] = e.toString();
                } finally {
                    latch.countDown();
                }
            }, 1);
            System.out.println("READY");
            System.out.flush();
            JsonObject out = new JsonObject();
            if (!latch.await(10, TimeUnit.SECONDS)) {
                out.addProperty("ok", false);
                out.addProperty("error", "timeout");
                System.out.println(out);
                System.exit(1);
            }
            boolean isBinary = box[0] != null && box[0].isBinaryBody();
            String hex = null;
            try {
                byte[] bytes = isBinary ? box[0].getBinaryBody() : null;
                hex = bytes == null ? null : HexFormat.of().formatHex(bytes);
            } catch (Exception e) {
                error[0] = e.toString();
            }
            boolean ok = isBinary && expectedHex.equals(hex);
            out.addProperty("ok", ok);
            out.addProperty("is_binary", isBinary);
            if (hex != null) {
                out.addProperty("hex", hex);
            }
            if (error[0] != null) {
                out.addProperty("error", error[0]);
            }
            System.out.println(out);
            prov.close();
            System.exit(ok ? 0 : 1);
        } else if (role.equals("binary-pub")) {
            String topic = args[1];
            byte[] bytes = HexFormat.of().parseHex(args[2]);
            StandaloneMessagingProvider prov = provider("binpub");
            Message msg = MessageBuilder.create("InteropBinary", "1.0")
                    .withPayload(bytes).build();
            prov.publish(topic, msg);
            Thread.sleep(500);
            prov.close();
        } else if (role.equals("typed-sub")) {
            String topic = args[1];
            String expectedHex = args[2].toLowerCase(java.util.Locale.ROOT);
            StandaloneMessagingProvider prov = provider("typedsub");
            final CountDownLatch latch = new CountDownLatch(1);
            final JsonObject[] result = new JsonObject[1];
            prov.subscribe(topic, (t, m) -> {
                JsonObject out = new JsonObject();
                try {
                    JsonObject body = (JsonObject) m.getBody();
                    JsonObject sample = body.getAsJsonArray("samples").get(0).getAsJsonObject();
                    String data = sample.getAsJsonObject("value")
                            .getAsJsonObject("_edgecommonsBinary")
                            .get("data").getAsString();
                    byte[] sampleBytes = Base64.getDecoder().decode(data);
                    out.addProperty("body_case", m.getBodyCase().name());
                    out.addProperty("hex", HexFormat.of().formatHex(sampleBytes));
                    out.addProperty("source_ts_ms", sample.get("sourceTsMs").getAsLong());
                    out.addProperty("server_ts_ms", sample.get("serverTsMs").getAsLong());
                    if (m.getTags() != null && m.getTags().toDict().containsKey("from")) {
                        out.addProperty("tag_from", m.getTags().toDict().get("from").getAsString());
                    }
                } catch (Exception e) {
                    out.addProperty("error", e.toString());
                }
                result[0] = out;
                latch.countDown();
            }, 1);
            System.out.println("READY");
            System.out.flush();
            if (!latch.await(10, TimeUnit.SECONDS)) {
                JsonObject out = new JsonObject();
                out.addProperty("ok", false);
                out.addProperty("error", "timeout");
                System.out.println(out);
                System.exit(1);
            }
            JsonObject out = result[0];
            boolean ok = MessageBodyCase.SOUTHBOUND_SIGNAL_UPDATE.name().equals(out.get("body_case").getAsString())
                    && expectedHex.equals(out.get("hex").getAsString())
                    && out.get("source_ts_ms").getAsLong() == 1783360799900L
                    && out.get("server_ts_ms").getAsLong() == 1783360800000L;
            out.addProperty("ok", ok);
            System.out.println(out);
            prov.close();
            System.exit(ok ? 0 : 1);
        } else if (role.equals("typed-pub")) {
            String topic = args[1];
            byte[] bytes = HexFormat.of().parseHex(args[2]);
            StandaloneMessagingProvider prov = provider("typedpub");
            JsonObject tags = new JsonObject();
            tags.addProperty("from", LANG);
            Message msg = MessageBuilder.create("SouthboundSignalUpdate", "1.0")
                    .withSouthboundSignalUpdate(typedBody(bytes))
                    .withTags(MessageTags.fromDict(tags))
                    .build();
            prov.publish(topic, msg);
            Thread.sleep(500);
            prov.close();
        } else if (role.equals("log-sub")) {
            String topic = args[1];
            String token = args[2];
            StandaloneMessagingProvider prov = provider("logsub");
            final CountDownLatch latch = new CountDownLatch(1);
            final JsonObject[] result = new JsonObject[1];
            prov.subscribe(topic, (t, m) -> {
                JsonObject out = new JsonObject();
                try {
                    JsonObject body = (JsonObject) m.getBody();
                    JsonObject identity = m.getIdentity() == null ? null : m.getIdentity().toDict();
                    JsonObject header = m.getHeader() == null ? null : m.getHeader().toDict();
                    JsonObject fields = body.has("fields") ? body.getAsJsonObject("fields") : new JsonObject();
                    boolean ok = t.equals(topic)
                            && "edgecommons.log.v1".equals(body.get("schema").getAsString())
                            && "WARN".equals(body.get("level").getAsString())
                            && ("log-interop-" + token).equals(body.get("message").getAsString())
                            && fields.has("nonce") && token.equals(fields.get("nonce").getAsString())
                            && identity != null
                            && "interop-device".equals(wireIdentityDevice(identity))
                            && identity.get("component").getAsString().startsWith("interop-log-")
                            // Component scope (D-U28): the wire identity omits `instance`.
                            && !identity.has("instance")
                            && header != null
                            && "log".equals(header.get("name").getAsString())
                            && "1.0".equals(header.get("version").getAsString());
                    out.addProperty("ok", ok);
                    out.addProperty("topic", t);
                    out.add("header", header == null ? JsonNull.INSTANCE : header);
                    out.add("identity", identity == null ? JsonNull.INSTANCE : identity);
                    out.add("body", body);
                } catch (Exception e) {
                    out.addProperty("ok", false);
                    out.addProperty("error", e.toString());
                }
                result[0] = out;
                latch.countDown();
            }, 1);
            System.out.println("READY");
            System.out.flush();
            if (!latch.await(10, TimeUnit.SECONDS)) {
                JsonObject out = new JsonObject();
                out.addProperty("ok", false);
                out.addProperty("error", "timeout");
                System.out.println(out);
                System.exit(1);
            }
            System.out.println(result[0]);
            prov.close();
            System.exit(result[0].has("ok") && result[0].get("ok").getAsBoolean() ? 0 : 1);
        } else if (role.equals("log-pub")) {
            String token = args[1];
            Path path = writeLogRuntimeConfig();
            EdgeCommons gg = null;
            try {
                gg = new EdgeCommons(
                        "com.mbreissi.edgecommons.interop." + LANG + ".LogPublisher",
                        logRuntimeArgs(path));
                gg.getLogs().publish(LogRecord.builder()
                        .withLevel("WARN")
                        .withLogger("interop." + LANG)
                        .withMessage("log-interop-" + token)
                        .withFields(Map.of("nonce", token, "publisher", LANG))
                        .build());
                boolean flushed = gg.getLogs().flush(Duration.ofSeconds(5));
                JsonObject out = new JsonObject();
                out.addProperty("ok", flushed && gg.getLogs().stats().getPublishedRecords() >= 1);
                out.addProperty("component", logComponentToken());
                out.addProperty("published", gg.getLogs().stats().getPublishedRecords());
                System.out.println(out);
                System.exit(out.get("ok").getAsBoolean() ? 0 : 1);
            } finally {
                if (gg != null) {
                    gg.shutdown();
                }
                Files.deleteIfExists(path);
            }
        } else if (role.equals("gg-log-matrix")) {
            String runId = args[1];
            String[] expectedLangs = args[2].split(",");
            String[] readyLangs = System.getenv().getOrDefault("EDGECOMMONS_GG_READY_LANGS", args[2]).split(",");
            String readyLang = System.getenv().getOrDefault("EDGECOMMONS_GG_READY_LANG", LANG);
            long subscribeDelayMs = (long) (Double.parseDouble(
                    System.getenv().getOrDefault("EDGECOMMONS_GG_SUBSCRIBE_DELAY_SECS", "8")) * 1000);
            long waitMs = (long) (Double.parseDouble(
                    System.getenv().getOrDefault("EDGECOMMONS_GG_WAIT_SECS", "35")) * 1000);
            MessagingProvider prov = new GreengrassMessagingProvider(true);
            java.util.concurrent.ConcurrentHashMap<String, JsonObject> received = new java.util.concurrent.ConcurrentHashMap<>();
            java.util.concurrent.ConcurrentHashMap<String, String> errors = new java.util.concurrent.ConcurrentHashMap<>();
            CountDownLatch latch = new CountDownLatch(expectedLangs.length);
            prov.subscribe("ecv1/interop-device/+/log/warn", (topic, m) -> {
                try {
                    JsonObject body = (JsonObject) m.getBody();
                    JsonObject identity = m.getIdentity() == null ? null : m.getIdentity().toDict();
                    String component = identity == null ? "" : identity.get("component").getAsString();
                    String publisher = component.startsWith("interop-log-")
                            ? component.substring("interop-log-".length())
                            : component;
                    JsonObject fields = body.has("fields") ? body.getAsJsonObject("fields") : new JsonObject();
                    boolean ok = java.util.Arrays.asList(expectedLangs).contains(publisher)
                            && "interop-device".equals(wireIdentityDevice(identity))
                            && identity != null
                            // D-U28: component-scope log record omits the instance token on
                            // the wire; its absence is the omit-when-absent proof over IPC.
                            && !identity.has("instance")
                            && "edgecommons.log.v1".equals(body.get("schema").getAsString())
                            && "WARN".equals(body.get("level").getAsString())
                            && ("interop." + publisher).equals(body.get("logger").getAsString())
                            && ("gg-log-interop-" + runId + "-" + publisher).equals(body.get("message").getAsString())
                            && fields.has("runId") && runId.equals(fields.get("runId").getAsString())
                            && fields.has("publisher") && publisher.equals(fields.get("publisher").getAsString());
                    JsonObject item = new JsonObject();
                    item.addProperty("ok", ok);
                    item.addProperty("topic", topic);
                    item.add("identity", identity == null ? JsonNull.INSTANCE : identity);
                    item.add("body", body);
                    if (received.putIfAbsent(publisher, item) == null) {
                        latch.countDown();
                    }
                } catch (Exception e) {
                    errors.put("log:" + topic, e.toString());
                }
            }, 1, 64);
            System.out.println("READY");
            System.out.flush();
            Files.writeString(ggLogReadyPath(runId, readyLang), Long.toString(System.currentTimeMillis()),
                    StandardCharsets.UTF_8);
            EdgeCommons gg = null;
            Path path = null;
            JsonObject published = new JsonObject();
            try {
                java.util.List<String> readyMissing = waitForGgLogReady(runId, readyLangs);
                Thread.sleep(subscribeDelayMs);
                if (readyMissing.isEmpty()) {
                    path = writeLogRuntimeConfig();
                    gg = new EdgeCommons(
                            "com.mbreissi.edgecommons.interop." + LANG + ".LogPublisher",
                            ggLogRuntimeArgs(path));
                    gg.getLogs().publish(LogRecord.builder()
                            .withLevel("WARN")
                            .withLogger("interop." + LANG)
                            .withMessage("gg-log-interop-" + runId + "-" + LANG)
                            .withFields(Map.of("runId", runId, "publisher", LANG))
                            .build());
                    gg.getLogs().flush(Duration.ofSeconds(5));
                    published.addProperty("published", gg.getLogs().stats().getPublishedRecords());
                    published.addProperty("failed", gg.getLogs().stats().getPublishFailures());
                }
                latch.await(waitMs, TimeUnit.MILLISECONDS);
                java.util.List<String> missing = new java.util.ArrayList<>();
                for (String lang : expectedLangs) {
                    if (!received.containsKey(lang)) {
                        missing.add(lang);
                    }
                }
                boolean allOk = true;
                for (String lang : expectedLangs) {
                    JsonObject item = received.get(lang);
                    allOk = allOk && item != null && item.has("ok") && item.get("ok").getAsBoolean();
                }
                boolean ok = readyMissing.isEmpty() && missing.isEmpty() && errors.isEmpty() && allOk;
                JsonObject result = new JsonObject();
                result.addProperty("ok", ok);
                result.addProperty("lang", LANG);
                result.addProperty("run_id", runId);
                result.add("ready_missing", GSON.toJsonTree(readyMissing));
                result.add("missing", GSON.toJsonTree(missing));
                result.add("received", GSON.toJsonTree(received));
                result.add("errors", GSON.toJsonTree(errors));
                result.add("published", published);
                Files.writeString(
                        Path.of("/tmp", "edgecommons_gg_ipc_log_" + readyLang + "_" + runId + ".json"),
                        GSON.toJson(result),
                        StandardCharsets.UTF_8);
                System.out.println(result);
                System.exit(ok ? 0 : 1);
            } finally {
                if (gg != null) {
                    gg.shutdown();
                }
                if (path != null) {
                    Files.deleteIfExists(path);
                }
                prov.close();
            }
        } else if (role.equals("gg-p1-matrix")) {
            runGgP1Matrix(args);
        } else if (role.equals("gg-binary-matrix")) {
            String runId = args[1];
            String[] expectedLangs = args[2].split(",");
            String[] readyLangs = System.getenv().getOrDefault("EDGECOMMONS_GG_READY_LANGS", args[2]).split(",");
            String readyLang = System.getenv().getOrDefault("EDGECOMMONS_GG_READY_LANG", LANG);
            String expectedHex = args[3].toLowerCase(java.util.Locale.ROOT);
            byte[] expectedBytes = HexFormat.of().parseHex(expectedHex);
            long subscribeDelayMs = (long) (Double.parseDouble(
                    System.getenv().getOrDefault("EDGECOMMONS_GG_SUBSCRIBE_DELAY_SECS", "8")) * 1000);
            long waitMs = (long) (Double.parseDouble(
                    System.getenv().getOrDefault("EDGECOMMONS_GG_WAIT_SECS", "35")) * 1000);
            MessagingProvider prov = new GreengrassMessagingProvider(true);
            java.util.concurrent.ConcurrentHashMap<String, JsonObject> received = new java.util.concurrent.ConcurrentHashMap<>();
            java.util.concurrent.ConcurrentHashMap<String, JsonObject> receivedTyped = new java.util.concurrent.ConcurrentHashMap<>();
            java.util.concurrent.ConcurrentHashMap<String, String> errors = new java.util.concurrent.ConcurrentHashMap<>();
            CountDownLatch latch = new CountDownLatch(expectedLangs.length * 2);
            prov.subscribe(ggTopic(runId, "+", LANG), (topic, m) -> {
                String publisher = publisherFromGgTopic(topic);
                if (publisher == null) publisher = "unknown";
                try {
                    boolean isBinary = m.isBinaryBody();
                    byte[] body = isBinary ? m.getBinaryBody() : null;
                    String hex = body == null ? null : HexFormat.of().formatHex(body);
                    boolean ok = isBinary && java.util.Arrays.equals(body, expectedBytes);
                    JsonObject item = new JsonObject();
                    item.addProperty("is_binary", isBinary);
                    if (hex != null) item.addProperty("hex", hex);
                    item.addProperty("ok", ok);
                    if (received.putIfAbsent(publisher, item) == null) {
                        latch.countDown();
                    }
                } catch (Exception e) {
                    errors.put(publisher + ":binary", e.toString());
                    JsonObject item = new JsonObject();
                    item.addProperty("is_binary", false);
                    item.addProperty("ok", false);
                    if (received.putIfAbsent(publisher, item) == null) {
                        latch.countDown();
                    }
                }
            }, 1, 64);
            prov.subscribe(ggTypedTopic(runId, "+", LANG), (topic, m) -> {
                String publisher = publisherFromGgTopic(topic);
                if (publisher == null) publisher = "unknown";
                try {
                    JsonObject body = (JsonObject) m.getBody();
                    JsonObject sample = body.getAsJsonArray("samples").get(0).getAsJsonObject();
                    String data = sample.getAsJsonObject("value")
                            .getAsJsonObject("_edgecommonsBinary")
                            .get("data").getAsString();
                    byte[] sampleBytes = Base64.getDecoder().decode(data);
                    String tagFrom = null;
                    if (m.getTags() != null && m.getTags().toDict().containsKey("from")) {
                        tagFrom = m.getTags().toDict().get("from").getAsString();
                    }
                    JsonObject item = new JsonObject();
                    item.addProperty("body_case", m.getBodyCase().name());
                    item.addProperty("hex", HexFormat.of().formatHex(sampleBytes));
                    item.addProperty("source_ts_ms", sample.get("sourceTsMs").getAsLong());
                    item.addProperty("server_ts_ms", sample.get("serverTsMs").getAsLong());
                    if (tagFrom != null) {
                        item.addProperty("tag_from", tagFrom);
                    }
                    boolean ok = MessageBodyCase.SOUTHBOUND_SIGNAL_UPDATE.equals(m.getBodyCase())
                            && java.util.Arrays.equals(sampleBytes, expectedBytes)
                            && sample.get("sourceTsMs").getAsLong() == 1783360799900L
                            && sample.get("serverTsMs").getAsLong() == 1783360800000L
                            && publisher.equals(tagFrom);
                    item.addProperty("ok", ok);
                    if (receivedTyped.putIfAbsent(publisher, item) == null) {
                        latch.countDown();
                    }
                } catch (Exception e) {
                    errors.put(publisher + ":typed", e.toString());
                    JsonObject item = new JsonObject();
                    item.addProperty("ok", false);
                    if (receivedTyped.putIfAbsent(publisher, item) == null) {
                        latch.countDown();
                    }
                }
            }, 1, 64);
            System.out.println("READY");
            System.out.flush();
            Files.writeString(ggReadyPath(runId, readyLang), Long.toString(System.currentTimeMillis()), StandardCharsets.UTF_8);
            java.util.List<String> readyMissing = waitForGgReady(runId, readyLangs);
            Thread.sleep(subscribeDelayMs);
            if (readyMissing.isEmpty()) {
                Message binaryMsg = MessageBuilder.create("InteropBinary", "1.0")
                        .withPayload(expectedBytes).build();
                JsonObject tags = new JsonObject();
                tags.addProperty("from", LANG);
                Message typedMsg = MessageBuilder.create("SouthboundSignalUpdate", "1.0")
                        .withSouthboundSignalUpdate(typedBody(expectedBytes))
                        .withTags(MessageTags.fromDict(tags))
                        .build();
                for (String target : expectedLangs) {
                    prov.publish(ggTopic(runId, LANG, target), binaryMsg);
                    prov.publish(ggTypedTopic(runId, LANG, target), typedMsg);
                }
            }
            latch.await(waitMs, TimeUnit.MILLISECONDS);
            JsonObject receivedJson = new JsonObject();
            for (Map.Entry<String, JsonObject> entry : received.entrySet()) {
                receivedJson.add(entry.getKey(), entry.getValue());
            }
            JsonObject receivedTypedJson = new JsonObject();
            for (Map.Entry<String, JsonObject> entry : receivedTyped.entrySet()) {
                receivedTypedJson.add(entry.getKey(), entry.getValue());
            }
            JsonObject errorsJson = new JsonObject();
            for (Map.Entry<String, String> entry : errors.entrySet()) {
                errorsJson.addProperty(entry.getKey(), entry.getValue());
            }
            com.google.gson.JsonArray readyMissingJson = new com.google.gson.JsonArray();
            for (String lang : readyMissing) {
                readyMissingJson.add(lang);
            }
            com.google.gson.JsonArray missing = new com.google.gson.JsonArray();
            com.google.gson.JsonArray missingTyped = new com.google.gson.JsonArray();
            boolean ok = errors.isEmpty() && readyMissing.isEmpty();
            for (String lang : expectedLangs) {
                if (!received.containsKey(lang)) {
                    missing.add(lang);
                    ok = false;
                } else if (!received.get(lang).get("ok").getAsBoolean()) {
                    ok = false;
                }
                if (!receivedTyped.containsKey(lang)) {
                    missingTyped.add(lang);
                    ok = false;
                } else if (!receivedTyped.get(lang).get("ok").getAsBoolean()) {
                    ok = false;
                }
            }
            JsonObject out = new JsonObject();
            out.addProperty("ok", ok);
            out.addProperty("lang", LANG);
            out.addProperty("run_id", runId);
            out.addProperty("expected_hex", expectedHex);
            out.add("ready_missing", readyMissingJson);
            out.add("received", receivedJson);
            out.add("received_typed", receivedTypedJson);
            out.add("missing", missing);
            out.add("missing_typed", missingTyped);
            out.add("errors", errorsJson);
            Files.writeString(
                    Path.of("/tmp", "edgecommons_gg_ipc_binary_" + LANG + "_" + runId + ".json"),
                    out.toString(), StandardCharsets.UTF_8);
            System.out.println(out);
            prov.close();
            System.exit(ok ? 0 : 1);
        } else if (role.equals("uns-pub")) {
            // uns-pub <identityJson> <class> [channel] — mint the topic with the real Uns
            // builder (includeRoot=false), stamp the identity via the real MessageBuilder,
            // publish, and print {"ok":true,"topic":...,"envelope":...}.
            MessageIdentity identity =
                    MessageIdentity.fromDict(GSON.fromJson(args[1], JsonObject.class));
            if (identity == null) {
                System.err.println("bad identity: " + args[1]);
                System.exit(2);
            }
            UnsClass cls = UnsClass.fromToken(args[2]);
            if (cls == null) {
                System.err.println("bad class: " + args[2]);
                System.exit(2);
            }
            String channel = args.length > 3 ? args[3] : null;
            Uns uns = new Uns(identity, false);
            String topic = channel == null ? uns.topic(cls) : uns.topic(cls, channel);
            StandaloneMessagingProvider prov = provider("unspub");
            JsonObject body = new JsonObject();
            body.addProperty("from", LANG);
            Message msg = MessageBuilder.create("UnsInterop", "1.0")
                    .withPayload(body).withIdentity(identity).build();
            prov.publish(topic, msg);
            Thread.sleep(500);
            JsonObject out = new JsonObject();
            out.addProperty("ok", true);
            out.addProperty("topic", topic);
            out.add("envelope", msg.toDict());
            System.out.println(out);
            prov.close();
        } else if (role.equals("uns-sub")) {
            // uns-sub <topic> — receive one envelope and print its parsed identity.
            String topic = args[1];
            StandaloneMessagingProvider prov = provider("unssub");
            final CountDownLatch latch = new CountDownLatch(1);
            final Message[] box = new Message[1];
            prov.subscribe(topic, (t, m) -> {
                box[0] = m;
                latch.countDown();
            }, 1);
            System.out.println("READY");
            System.out.flush();
            JsonObject out = new JsonObject();
            if (!latch.await(10, TimeUnit.SECONDS)) {
                out.addProperty("ok", false);
                out.addProperty("error", "timeout");
                System.out.println(out);
                System.exit(1);
            }
            MessageIdentity identity = box[0].getIdentity();
            boolean ok = identity != null;
            out.addProperty("ok", ok);
            out.add("identity", identity == null ? null : identity.toDict());
            out.add("body", asElement(box[0].getBody()));
            System.out.println(out);
            prov.close();
            System.exit(ok ? 0 : 1);
        } else if (role.equals("uns-guard")) {
            // uns-guard — the reserved-class guard lives on MessagingClient (§4.1) and
            // fires BEFORE the underlying provider is touched, so the protected test
            // constructor (null provider, guard intact) proves the real guard without a
            // broker connection.
            MessagingClient client = new MessagingClient() { };
            // Reserved-class target selectable (D-U28): instance-scoped default or the
            // component-scoped ecv1/dev1/comp1/state — the guard must reject both.
            String topic = args.length > 1 ? args[1] : "ecv1/dev1/comp1/main/state";
            JsonObject payload = new JsonObject();
            payload.addProperty("from", LANG);
            try {
                client.publishRaw(topic, payload);
            } catch (ReservedTopicException e) {
                JsonObject out = new JsonObject();
                out.addProperty("error", "ReservedTopicException");
                out.addProperty("class", e.getClassToken());
                out.addProperty("topic", e.getTopic());
                System.out.println(out);
                System.exit(3);
            }
            JsonObject out = new JsonObject();
            out.addProperty("ok", true);
            System.out.println(out);
            System.exit(0);
        } else if (role.equals("status-responder")) {
            // status-responder <component> — a real component whose built-in command inbox serves
            // the `status` verb from the registered per-instance connectivity provider (the PULL
            // surface). The heartbeat is off: `status` samples the provider directly, so this role
            // proves the pull path does not depend on the keepalive having ticked.
            String componentToken = args[1];
            Path path = writeConnectivityRuntimeConfig(componentToken, false);
            EdgeCommons gg = null;
            try {
                gg = new EdgeCommons(
                        "com.mbreissi.edgecommons.interop." + LANG + ".StatusResponder",
                        logRuntimeArgs(path));
                gg.setInstanceConnectivityProvider(InteropNode::canonicalInstances);
                CommandInbox inbox = gg.getCommands();
                if (inbox == null) {
                    throw new IllegalStateException("runtime did not expose command inbox");
                }
                System.out.println("READY");
                System.out.flush();
                Thread.sleep(Long.MAX_VALUE);
            } finally {
                if (gg != null) {
                    gg.shutdown();
                }
                Files.deleteIfExists(path);
            }
        } else if (role.equals("status-request")) {
            // status-request <component> — PULL the peer's built-in `status` verb over its own
            // command inbox and print the verb's result body.
            String component = args[1];
            String topic = commandTopic(component, CommandInbox.STATUS);
            StandaloneMessagingProvider prov = provider("statusreq");
            JsonObject out = new JsonObject();
            try {
                JsonObject requestBody = new JsonObject();
                requestBody.addProperty("from", LANG);
                Message request = MessageBuilder.create(CommandInbox.STATUS, "1.0")
                        .withCommand(requestBody).build();
                Message reply = prov.request(topic, request).get(20, TimeUnit.SECONDS);
                JsonElement replyBody = asElement(reply.getBody());
                JsonObject envelope = replyBody.isJsonObject() ? replyBody.getAsJsonObject() : null;
                JsonObject result = envelope != null && envelope.has("result")
                        && envelope.get("result").isJsonObject()
                        ? envelope.getAsJsonObject("result") : null;
                boolean ok = envelope != null && envelope.has("ok")
                        && envelope.get("ok").getAsBoolean() && result != null;
                out.addProperty("ok", ok);
                out.add("reply_body", result != null ? result : replyBody);
                System.out.println(out);
                prov.close();
                System.exit(ok ? 0 : 1);
            } catch (Exception e) {
                out.addProperty("ok", false);
                out.addProperty("error", e.getClass().getSimpleName());
                System.out.println(out);
                System.exit(1);
            }
        } else if (role.equals("state-instances-pub")) {
            // state-instances-pub <component> — a real component with the HEARTBEAT ENABLED and the
            // same provider registered, so the library PUSHES the sample in every RUNNING `state`
            // keepalive's instances[].
            String componentToken = args[1];
            Path path = writeConnectivityRuntimeConfig(componentToken, true);
            EdgeCommons gg = null;
            try {
                gg = new EdgeCommons(
                        "com.mbreissi.edgecommons.interop." + LANG + ".StateInstancesPublisher",
                        logRuntimeArgs(path));
                gg.setInstanceConnectivityProvider(InteropNode::canonicalInstances);
                System.out.println("READY");
                System.out.flush();
                Thread.sleep(Long.MAX_VALUE);
            } finally {
                if (gg != null) {
                    gg.shutdown();
                }
                Files.deleteIfExists(path);
            }
        } else if (role.equals("state-instances-sub")) {
            // state-instances-sub <component> — subscribe the peer's reserved `state` topic
            // (subscribing to a reserved class is allowed; only PUBLISHING is rejected) and settle
            // on the first RUNNING keepalive that carries a non-empty instances[].
            String component = args[1];
            String topic = stateTopic(component);
            StandaloneMessagingProvider prov = provider("statesub");
            final CountDownLatch latch = new CountDownLatch(1);
            final JsonObject[] box = new JsonObject[1];
            prov.subscribe(topic, (t, m) -> {
                try {
                    JsonElement bodyElement = asElement(m.getBody());
                    if (!bodyElement.isJsonObject()) {
                        return;
                    }
                    JsonObject body = bodyElement.getAsJsonObject();
                    if (!body.has("status")
                            || !"RUNNING".equals(body.get("status").getAsString())) {
                        return;  // the STOPPED farewell carries no live instances
                    }
                    if (!body.has("instances") || !body.get("instances").isJsonArray()
                            || body.getAsJsonArray("instances").isEmpty()) {
                        return;  // a tick sampled before the provider was registered
                    }
                    JsonObject out = new JsonObject();
                    out.addProperty("ok", true);
                    out.addProperty("state_status", "RUNNING");
                    out.add("instances", body.getAsJsonArray("instances"));
                    box[0] = out;
                    latch.countDown();
                } catch (Exception ignored) {
                    // A malformed/foreign envelope on the state topic is not this node's business.
                }
            }, 1);
            System.out.println("READY");
            System.out.flush();
            if (!latch.await(35, TimeUnit.SECONDS)) {
                JsonObject out = new JsonObject();
                out.addProperty("ok", false);
                out.addProperty("error", "timeout");
                System.out.println(out);
                System.exit(1);
            }
            System.out.println(box[0]);
            prov.close();
            System.exit(0);
        } else {
            System.err.println("unknown role: " + role);
            System.exit(2);
        }
    }
}
