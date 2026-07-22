package <<PACKAGE>>;

import com.mbreissi.edgecommons.EdgeCommons;
import com.mbreissi.edgecommons.EdgeCommonsBuilder;
import com.mbreissi.edgecommons.commands.CommandException;
import com.mbreissi.edgecommons.config.ConfigManager;
import com.mbreissi.edgecommons.facades.DataFacade;
import com.mbreissi.edgecommons.facades.EventsFacade;
import com.mbreissi.edgecommons.facades.Severity;
import com.mbreissi.edgecommons.heartbeat.InstanceConnectivity;
import com.mbreissi.edgecommons.messaging.Message;
import com.mbreissi.edgecommons.messaging.MessageBuilder;
import com.mbreissi.edgecommons.messaging.MessagingClient;
import com.mbreissi.edgecommons.metrics.MetricBuilder;
import com.mbreissi.edgecommons.metrics.MetricEmitter;
import com.mbreissi.edgecommons.uns.UnsClass;
import com.google.gson.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.atomic.AtomicReference;

import static com.mbreissi.edgecommons.utils.Utils.sleep;

/**
 * Minimal EdgeCommons component scaffold.
 *
 * <p>The library gives you config, messaging, metrics, logging and lifecycle; identity is
 * <b>config-driven</b>: the top-level {@code hierarchy} + {@code identity} config blocks resolve
 * to this component's UNS identity (the last hierarchy level is always the resolved thing name).
 * Every envelope built with {@code .withConfig(...)} carries that identity automatically, and
 * every topic is minted through {@code gg.getUns()} (or, for the app-usable classes below, by the
 * publish facade itself) — never hand-written.
 *
 * <p>The {@code state} heartbeat keepalive AND the component command inbox are both
 * <b>automatic</b> (library-owned, no code here): the {@code state} keepalive publishes on
 * {@code ecv1/{device}/{component}/main/state} (on / 5 s / local by default), and the inbox
 * ({@code ecv1/{device}/{component}/main/cmd/#}, {@code gg.getCommands()}) answers
 * {@code ping} / {@code status} / {@code describe} / {@code reload-config} /
 * {@code get-configuration} once its transport subscription is acknowledged.
 *
 * <p>What this scaffold adds is the rest of the monitoring + command surface the edge-console
 * reads (DESIGN-uns §7/§9 — G-S1/S2), so a freshly generated component has something to show up
 * on the console's Signals/Events/Metrics tabs and something custom to command, instead of an
 * empty dashboard:
 * <ul>
 *   <li>a periodic <b>metric</b> ({@value #METRIC_NAME}: a monotonic {@code tickCount} counter
 *       plus an {@code uptimeSecs} gauge-like measure) via {@code gg.getMetrics()};</li>
 *   <li>a periodic <b>data</b> signal ({@value #DATA_SIGNAL_ID}: a sine-wave demo reading) via
 *       {@code gg.getData()} — the {@link DataFacade} constructs the
 *       {@code SouthboundSignalUpdate} body (device/signal/samples) and defaults an omitted
 *       sample quality to {@code GOOD}, so the console's Signals tab has something to chart;</li>
 *   <li>a periodic <b>evt</b> ({@code ecv1/.../evt/info/sample-event}) via {@code gg.getEvents()}
 *       — the {@link EventsFacade} derives the {@code evt/{severity}/{type}} channel from the
 *       body's own severity + type, so the topic and body can never disagree;</li>
 *   <li>a custom <b>command verb</b> ({@value #SET_GREETING}), registered with
 *       {@code EdgeCommonsBuilder.configureCommands(...)} alongside the automatic built-ins, that mutates a
 *       small piece of in-memory state which the periodic {@code app} status publish below then
 *       reflects on its very next tick — so invoking it from the console is visibly observable.</li>
 * </ul>
 * Replace all four with your own business metrics/signals/events/verbs; none of this is required
 * by the library (a bare scaffold works fine without them), it exists so the demonstrated surface
 * is live end-to-end out of the box.
 */
public class <<COMPONENTNAME>>
{
    private static final Logger LOGGER = LogManager.getLogger(<<COMPONENTNAME>>.class);

    /** The demo loop-tick metric name (see the class docs). */
    private static final String METRIC_NAME = "loopTicks";
    /** The demo data() signal id (see the class docs). */
    private static final String DATA_SIGNAL_ID = "demo-signal";
    /** The custom command verb this scaffold registers (see the class docs). */
    private static final String SET_GREETING = "set-greeting";

    final EdgeCommons edgeCommons;
    final ConfigManager configManager;
    final MessagingClient messaging;
    final MetricEmitter metrics;
    /** The {@code data()} publish facade — see the class docs. */
    final DataFacade data;
    /** The {@code events()} publish facade — see the class docs. */
    final EventsFacade events;

    /**
     * In-memory demo state + its command handler: mutated by the {@value #SET_GREETING} command,
     * read back by the periodic {@code app} status publish — so a console "Send command" has a
     * visible effect without needing a dedicated custom "get" verb (the built-in
     * {@code get-configuration} already covers reading config back). The parse/validate/swap logic
     * lives in {@link Greeting} (a covered, broker-free unit) rather than inline here, so the one
     * piece of real logic in this scaffold is unit-tested while this class stays pure bootstrap +
     * run loop (see {@code Greeting} and the JaCoCo exclude note in {@code pom.xml}).
     */
    private final Greeting greeting = new Greeting("Hello from <<COMPONENTNAME>>");

    public static void main(String[] args) {
        new <<COMPONENTNAME>>(args);
    }

    public <<COMPONENTNAME>>(String[] args)
    {
        edgeCommons = EdgeCommonsBuilder.create("<<COMPONENTFULLNAME>>")
                .withArgs(args)
                .initialReady(false)
                // Install component verbs before the command-inbox subscription can become ACTIVE.
                .configureCommands(inbox -> inbox.register(
                        SET_GREETING, greeting::apply))
                .build();
        configManager = edgeCommons.getConfigManager();
        messaging = edgeCommons.getMessaging();
        metrics = edgeCommons.getMetrics();
        data = edgeCommons.getData();
        events = edgeCommons.getEvents();

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

        // --- per-instance connectivity: ONE provider, TWO surfaces. The library pushes this
        // sample into the `state` keepalive's instances[] every tick AND returns it from the
        // built-in `status` verb when pulled, so a console that subscribes and a console that
        // asks can never disagree. See instanceConnectivity() below.
        edgeCommons.setInstanceConnectivityProvider(<<COMPONENTNAME>>::instanceConnectivity);


        // The resolved UNS identity path (e.g. "site1/my-gw") and the topic minted from it. APP
        // is the free application class for this scaffold's status publish below; the data()
        // and events() facades mint their OWN topics from the signal id / severity+type - never
        // hand-write those.
        String statusTopic = edgeCommons.getUns().topic(UnsClass.APP, "status");
        LOGGER.info("UNS identity path: {} - status={}",
                edgeCommons.getUns().identity().getPath(), statusTopic);

        // All required handlers and metric definitions now exist; release the application gate.
        // Readiness still also requires connected messaging and an ACTIVE command inbox.
        edgeCommons.setReady(true);

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

            // 3) data - a periodic sample telemetry signal (the console's Signals tab), through
            // the data() facade: it constructs the SouthboundSignalUpdate body
            // (device/signal/samples), sanitizes the channel, and stamps identity - a real
            // adapter maps one protocol read onto addSample(...) and never touches the envelope
            // or topic (DESIGN-class-facades §2.1). A sine wave stands in for a live sensor
            // reading here; addSample(value) with no explicit Quality demonstrates the facade's
            // honest default - an unspecified reading defaults to Quality.GOOD (marked
            // qualityRaw="unspecified" on the wire so a consumer can tell a synthesized GOOD
            // from a device-reported one). Pass an explicit Quality.BAD/UNCERTAIN when your
            // source knows a read failed or is stale.
            double demoValue = 20.0 + 5.0 * Math.sin(seq / 10.0);
            data.signal(DATA_SIGNAL_ID)
                    .name("Demo Signal")
                    .addSample(demoValue)
                    .publish();

            // 4) evt - a discrete, human-meaningful occurrence (not a metric, not liveness
            // state); the console's Events tab. Through the events() facade: emit(severity,
            // type, message, context) derives the evt/{severity}/{type} channel from the body's
            // own severity + type, so the topic and body can never disagree
            // (DESIGN-class-facades §2.2) - no more hand-built body/topic. A real component
            // would emit these on actual occurrences (a threshold crossed, a connection
            // lost/restored, ...), not on a fixed timer; raiseAlarm/clearAlarm are there for
            // stateful alarms (e.g. a connection-lost/connection-restored pair).
            JsonObject context = new JsonObject();
            context.addProperty("seq", seq);
            context.addProperty("greeting", greeting.get());
            events.emit(Severity.INFO, "sample-event", "sample event from <<COMPONENTNAME>>", context);

            LOGGER.info("Running... (seq={} uptimeSecs={} greeting='{}')", seq, uptimeSecs, greeting.get());
            sleep(10000);
        }
    }

    /**
     * The per-instance connectivity this component reports — <b>none</b>, because a service owns no
     * southbound connections. A component with no instances reports none: its {@code state}
     * keepalive carries no {@code instances[]} section, and the built-in {@code status} verb answers
     * exactly as {@code ping} does ({@code {"status":"RUNNING","uptimeSecs":n}}). That is the honest
     * answer, not a gap.
     *
     * <p>If this component <i>does</i> own connections (a database pool, an upstream HTTP API, a
     * device session), return one entry per connection instead — each entry is a cached status read,
     * never live IO: this runs on the heartbeat thread every tick.
     *
     * <pre>{@code
     * return List.of(InstanceConnectivity.of("db-1", pool.isUp(), "postgres://…")
     *         .withState("BACKOFF")                                            // OUR vocabulary
     *         .withAttributes(Map.of("lastError", new JsonPrimitive("timeout")))); // domain data
     * }</pre>
     *
     * <p>{@code connected} is the one <b>normalized</b> field and is always present, so any console
     * can render a health dot for any component without knowing that component's vocabulary.
     * {@code state} is the component's <i>own</i> token for what a boolean cannot say ("reconnecting"
     * vs "administratively disabled"), and {@code attributes} is an open bag: domain data goes there,
     * where it can never destabilize the fields every consumer relies on.
     */
    static List<InstanceConnectivity> instanceConnectivity()
    {
        return List.of();
    }

}

/**
 * The mutable demo greeting <b>and</b> its {@code set-greeting} command handler — the one piece of
 * real, unit-testable logic in this scaffold. It is a top-level, dependency-free class on purpose:
 * the enclosing component is a live bootstrap + infinite run loop (it needs a broker and a running
 * {@code EdgeCommons} to do anything), so that class is validated on real infrastructure and excluded
 * from the in-process coverage gate — but the parse/validate/swap logic here needs none of that, so
 * it lives where a plain JUnit test can cover it. Replace this with your own command state as you
 * build the component out; keep it testable in the same way.
 */
final class Greeting {

    private final AtomicReference<String> value;

    Greeting(String initial) {
        this.value = new AtomicReference<>(initial);
    }

    /** The current greeting — what the periodic {@code app} status publish reflects each tick. */
    String get() {
        return value.get();
    }

    /**
     * The {@code set-greeting} custom command verb: {@code {"greeting": "<new text>"}} in,
     * {@code {"previousGreeting": ..., "greeting": ...}} out. Throws a {@link CommandException}
     * (a coded error reply, {@code BAD_ARGS}) on a missing/malformed argument, exactly like the
     * library's own built-ins do for their failure modes.
     *
     * <p>Try it from the CLI (fire-and-forget doesn't need a reply_to; the inbox still runs the
     * handler): publish {@code {"header":{"name":"set-greeting","version":"1.0"},"body":
     * {"greeting":"Hi from mqttx"}}} to {@code ecv1/{device}/{component}/main/cmd/set-greeting}.
     */
    JsonObject apply(Message request) throws CommandException {
        // Pattern-matching instanceof (JEP 394, Java 16+) — fine on this template's Java 25 target.
        if (!(request.getBody() instanceof JsonObject body) || !body.has("greeting")
                || !body.get("greeting").isJsonPrimitive()) {
            throw new CommandException("BAD_ARGS", "expected a JSON body {\"greeting\": \"<text>\"}");
        }
        String next = body.get("greeting").getAsString();
        String previous = value.getAndSet(next);
        JsonObject result = new JsonObject();
        result.addProperty("previousGreeting", previous);
        result.addProperty("greeting", next);
        return result;
    }
}
