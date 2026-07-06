import com.mbreissi.edgecommons.messaging.Message;
import com.mbreissi.edgecommons.messaging.MessageBuilder;
import com.mbreissi.edgecommons.messaging.MessageIdentity;
import com.mbreissi.edgecommons.messaging.MessagingClient;
import com.mbreissi.edgecommons.messaging.MessagingConfiguration;
import com.mbreissi.edgecommons.messaging.ReservedTopicException;
import com.mbreissi.edgecommons.messaging.providers.standalone.StandaloneMessagingProvider;
import com.mbreissi.edgecommons.uns.Uns;
import com.mbreissi.edgecommons.uns.UnsClass;
import com.google.gson.Gson;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;

import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;

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
