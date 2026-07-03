package <<PACKAGE>>;

import com.mbreissi.ggcommons.GGCommons;
import com.mbreissi.ggcommons.GGCommonsBuilder;
import com.mbreissi.ggcommons.commands.CommandException;
import com.mbreissi.ggcommons.commands.CommandInbox;
import com.mbreissi.ggcommons.config.ConfigManager;
import com.mbreissi.ggcommons.messaging.Message;
import com.mbreissi.ggcommons.messaging.MessageBuilder;
import com.mbreissi.ggcommons.messaging.MessagingClient;
import com.mbreissi.ggcommons.metrics.MetricBuilder;
import com.mbreissi.ggcommons.metrics.MetricEmitter;
import com.mbreissi.ggcommons.uns.UnsClass;
import com.google.gson.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.HashMap;
import java.util.Map;
import java.util.concurrent.atomic.AtomicReference;

import static com.mbreissi.ggcommons.utils.Utils.sleep;

/**
 * Minimal GGCommons component scaffold.
 *
 * <p>The library gives you config, messaging, metrics, logging and lifecycle; identity is
 * <b>config-driven</b>: the top-level {@code hierarchy} + {@code identity} config blocks resolve
 * to this component's UNS identity (the last hierarchy level is always the resolved thing name).
 * Every envelope built with {@code .withConfig(...)} carries that identity automatically, and
 * every topic is minted through {@code gg.getUns()} — never hand-written.
 *
 * <p>The {@code state} heartbeat keepalive AND the component command inbox are both
 * <b>automatic</b> (library-owned, no code here): the {@code state} keepalive publishes on
 * {@code ecv1/{device}/{component}/main/state} (on / 5 s / local by default), and the inbox
 * ({@code ecv1/{device}/{component}/main/cmd/#}, {@code gg.getCommands()}) already answers
 * {@code ping} / {@code reload-config} / {@code get-configuration} before this constructor even
 * runs.
 *
 * <p>What this scaffold adds is the rest of the monitoring + command surface the edge-console
 * reads (DESIGN-uns §7/§9 — G-S1/S2), so a freshly generated component has something to show up
 * on the console's Events/Metrics tabs and something custom to command, instead of an empty
 * dashboard:
 * <ul>
 *   <li>a periodic <b>metric</b> ({@value #METRIC_NAME}: a monotonic {@code tickCount} counter
 *       plus an {@code uptimeSecs} gauge-like measure) via {@code gg.getMetrics()};</li>
 *   <li>a periodic <b>evt</b> ({@code ecv1/.../evt/sample-event}) via the UNS topic builder +
 *       {@code MessageBuilder} — there is no dedicated {@code events()} facade yet, so an evt is
 *       just a normal published message on the open {@code evt} class;</li>
 *   <li>a custom <b>command verb</b> ({@value #SET_GREETING}), registered with
 *       {@code gg.getCommands().register(...)} alongside the automatic built-ins, that mutates a
 *       small piece of in-memory state which the periodic {@code app} status publish below then
 *       reflects on its very next tick — so invoking it from the console is visibly observable.</li>
 * </ul>
 * Replace all three with your own business metrics/events/verbs; none of this is required by the
 * library (a bare scaffold works fine without them), it exists so the demonstrated surface is
 * live end-to-end out of the box.
 */
public class <<COMPONENTNAME>>
{
    private static final Logger LOGGER = LogManager.getLogger(<<COMPONENTNAME>>.class);

    /** The demo loop-tick metric name (see the class docs). */
    private static final String METRIC_NAME = "loopTicks";
    /** The custom command verb this scaffold registers (see the class docs). */
    private static final String SET_GREETING = "set-greeting";

    final GGCommons ggCommons;
    final ConfigManager configManager;
    final MessagingClient messaging;
    final MetricEmitter metrics;
    final CommandInbox commands;

    /**
     * In-memory demo state: mutated by the {@value #SET_GREETING} command, read back by the
     * periodic {@code app} status publish — so a console "Send command" has a visible effect
     * without needing a dedicated custom "get" verb (the built-in {@code get-configuration}
     * already covers reading config back).
     */
    private final AtomicReference<String> greeting = new AtomicReference<>("Hello from <<COMPONENTNAME>>");

    public static void main(String[] args) {
        new <<COMPONENTNAME>>(args);
    }

    public <<COMPONENTNAME>>(String[] args)
    {
        ggCommons = GGCommonsBuilder.create("<<COMPONENTFULLNAME>>").withArgs(args).build();
        configManager = ggCommons.getConfigManager();
        messaging = ggCommons.getMessaging();
        metrics = ggCommons.getMetrics();
        commands = ggCommons.getCommands();

        // --- metrics: define once, emit every tick. MetricBuilder is the sanctioned
        // construction path (never `new Metric(...)`, deprecated). Two measures show a metric
        // isn't just a single scalar: a monotonic counter (tickCount) and a gauge-like elapsed
        // value (uptimeSecs); addDimension adds a custom EMF/CloudWatch dimension on top of the
        // library's own default coreName/component dimensions.
        metrics.defineMetric(MetricBuilder.create(METRIC_NAME)
                .withConfig(configManager)
                .addMeasure("tickCount", "Count", 60)
                .addMeasure("uptimeSecs", "Seconds", 60)
                .addDimension("demo", "scaffold")
                .build());

        // --- commands: ping/reload-config/get-configuration are already live (wired by the
        // library before this constructor runs). Register ONE custom verb so there is something
        // for the console's "Send command" to invoke beyond the built-ins. `getCommands()` is
        // only null on a mock/subclass bring-up that never initialized - guard defensively.
        if (commands != null) {
            commands.register(SET_GREETING, this::handleSetGreeting);
        }

        // The resolved UNS identity path (e.g. "site1/my-gw") and the topics minted from it.
        // APP is the free application class; EVT is for discrete, notable occurrences (this
        // scaffold's sample event) — metric publishes go through gg.getMetrics() above, never a
        // hand-built topic.
        String statusTopic = ggCommons.getUns().topic(UnsClass.APP, "status");
        String eventTopic = ggCommons.getUns().topic(UnsClass.EVT, "sample-event");
        LOGGER.info("UNS identity path: {} - status={} event={}",
                ggCommons.getUns().identity().getPath(), statusTopic, eventTopic);

        long seq = 0;
        long startMillis = System.currentTimeMillis();
        while (true)
        {
            seq++;
            long uptimeSecs = (System.currentTimeMillis() - startMillis) / 1000;

            // 1) app status - reflects the current greeting (mutable via the set-greeting command
            // above), so a console operator can watch a command's effect land on the next tick.
            JsonObject statusBody = new JsonObject();
            statusBody.addProperty("seq", seq);
            statusBody.addProperty("message", greeting.get());
            // .withConfig(...) stamps the envelope's `identity` block (instance "main")
            // automatically; building without it is legal only for identity-free bootstrap/raw
            // messages.
            Message statusMsg = MessageBuilder.create("StatusUpdate", "1.0")
                    .withPayload(statusBody)
                    .withConfig(configManager)
                    .build();
            messaging.publish(statusTopic, statusMsg);

            // 2) metric - a loop-tick counter plus an uptime-ish gauge (the console's Metrics tab).
            Map<String, Float> measures = new HashMap<>();
            measures.put("tickCount", (float) seq);
            measures.put("uptimeSecs", (float) uptimeSecs);
            metrics.emitMetric(METRIC_NAME, measures);

            // 3) evt - a discrete, human-meaningful occurrence (not a metric, not liveness state);
            // the console's Events tab. A real component would emit these on actual occurrences
            // (a threshold crossed, a connection lost/restored, ...), not on a fixed timer.
            JsonObject eventBody = new JsonObject();
            eventBody.addProperty("severity", "info");
            eventBody.addProperty("message", "sample event from <<COMPONENTNAME>>");
            JsonObject context = new JsonObject();
            context.addProperty("seq", seq);
            context.addProperty("greeting", greeting.get());
            eventBody.add("context", context);
            Message eventMsg = MessageBuilder.create("SampleEvent", "1.0")
                    .withPayload(eventBody)
                    .withConfig(configManager)
                    .build();
            messaging.publish(eventTopic, eventMsg);

            LOGGER.info("Running... (seq={} uptimeSecs={} greeting='{}')", seq, uptimeSecs, greeting.get());
            sleep(10000);
        }
    }

    /**
     * The {@value #SET_GREETING} custom command verb: {@code {"greeting": "<new text>"}} in,
     * {@code {"previousGreeting": ..., "greeting": ...}} out. Throws a {@link CommandException}
     * (a coded error reply, {@code BAD_ARGS}) on a missing/malformed argument, exactly like the
     * library's own built-ins do for their failure modes.
     *
     * <p>Try it from the CLI (fire-and-forget doesn't need a reply_to; the inbox still runs the
     * handler): publish {@code {"header":{"name":"set-greeting","version":"1.0"},"body":
     * {"greeting":"Hi from mqttx"}}} to {@code ecv1/{device}/{component}/main/cmd/set-greeting}.
     */
    private JsonObject handleSetGreeting(Message request) throws CommandException {
        // Pattern-matching instanceof (JEP 394, Java 16+) — fine on this template's Java 25 target.
        if (!(request.getBody() instanceof JsonObject body) || !body.has("greeting")
                || !body.get("greeting").isJsonPrimitive()) {
            throw new CommandException("BAD_ARGS", "expected a JSON body {\"greeting\": \"<text>\"}");
        }
        String next = body.get("greeting").getAsString();
        String previous = greeting.getAndSet(next);
        JsonObject result = new JsonObject();
        result.addProperty("previousGreeting", previous);
        result.addProperty("greeting", next);
        return result;
    }
}
