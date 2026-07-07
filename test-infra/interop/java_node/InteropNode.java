import com.mbreissi.edgecommons.messaging.Message;
import com.mbreissi.edgecommons.messaging.MessageBuilder;
import com.mbreissi.edgecommons.messaging.MessageIdentity;
import com.mbreissi.edgecommons.messaging.MessagingClient;
import com.mbreissi.edgecommons.messaging.MessagingConfiguration;
import com.mbreissi.edgecommons.messaging.MessagingProvider;
import com.mbreissi.edgecommons.messaging.MessageTags;
import com.mbreissi.edgecommons.messaging.ReservedTopicException;
import com.mbreissi.edgecommons.messaging.proto.MessageBodyCase;
import com.mbreissi.edgecommons.messaging.providers.greengrass.GreengrassMessagingProvider;
import com.mbreissi.edgecommons.messaging.providers.standalone.StandaloneMessagingProvider;
import com.mbreissi.edgecommons.uns.Uns;
import com.mbreissi.edgecommons.uns.UnsClass;
import com.google.gson.Gson;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;

import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.Base64;
import java.util.HexFormat;
import java.util.Map;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.charset.StandardCharsets;

/**
 * Cross-language interop node (Java) for edgecommons. Shared CLI contract:
 *   InteropNode responder &lt;request_topic&gt;
 *   InteropNode request   &lt;request_topic&gt; &lt;token&gt;
 *   InteropNode uns-pub   &lt;identityJson&gt; &lt;class&gt; [channel]
 *   InteropNode uns-sub   &lt;topic&gt;
 *   InteropNode uns-guard
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
            String topic = "ecv1/dev1/comp1/main/state";
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
        } else {
            System.err.println("unknown role: " + role);
            System.exit(2);
        }
    }
}
