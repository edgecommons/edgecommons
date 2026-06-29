import com.mbreissi.ggcommons.config.ConfigManager;
import com.mbreissi.ggcommons.messaging.Message;
import com.mbreissi.ggcommons.messaging.MessageBuilder;
import com.mbreissi.ggcommons.messaging.MessagingConfiguration;
import com.mbreissi.ggcommons.messaging.providers.standalone.StandaloneMessagingProvider;
import com.google.gson.Gson;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;

import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;

/**
 * Cross-language interop node (Java) for ggcommons. Shared CLI contract:
 *   InteropNode responder &lt;request_topic&gt;
 *   InteropNode request   &lt;request_topic&gt; &lt;token&gt;
 * Local-only MQTT transport against localhost:1883.
 */
public class InteropNode {
    static final String LANG = "java";
    static final Gson GSON = new Gson();

    /** Minimal ConfigManager so MessageBuilder.withConfig(...) works standalone. */
    static class InteropConfig extends ConfigManager {
        private final String thing;
        InteropConfig(String thing) { super(); this.thing = thing; }
        @Override public String getThingName() { return thing; }
    }

    static String host() {
        String h = System.getenv("GGCOMMONS_IT_MQTT_HOST");
        return h != null ? h : "localhost";
    }

    static String port() {
        String p = System.getenv("GGCOMMONS_IT_MQTT_PORT");
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
        ConfigManager cfg = new InteropConfig("interop-java");

        if (role.equals("responder")) {
            String topic = args[1];
            StandaloneMessagingProvider prov = provider("resp");
            prov.subscribe(topic, (t, request) -> {
                JsonObject body = new JsonObject();
                body.add("echo", asElement(request.getBody()));
                body.addProperty("responder", LANG);
                Message reply = MessageBuilder.create("InteropReply", "1.0")
                        .withPayload(body).withConfig(cfg).build();
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
                    .withPayload(reqBody).withConfig(cfg).build();
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
                out.addProperty("ok", false);
                out.addProperty("error", "timeout");
                System.out.println(out);
                System.exit(1);
            }
            Object raw = box[0].getRaw();
            boolean isRaw = raw != null;
            String rawToken = null;
            JsonElement rawEl = isRaw ? asElement(raw) : null;
            if (rawEl != null && rawEl.isJsonObject() && rawEl.getAsJsonObject().has("token")) {
                rawToken = rawEl.getAsJsonObject().get("token").getAsString();
            }
            boolean ok = isRaw && token.equals(rawToken);
            out.addProperty("ok", ok);
            out.addProperty("is_raw", isRaw);
            if (rawToken != null) {
                out.addProperty("raw_token", rawToken);
            }
            System.out.println(out);
            prov.close();
            System.exit(ok ? 0 : 1);
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
        } else {
            System.err.println("unknown role: " + role);
            System.exit(2);
        }
    }
}
