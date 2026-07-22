package <<PACKAGE>>;

import com.google.gson.JsonObject;
import com.mbreissi.edgecommons.EdgeCommons;
import com.mbreissi.edgecommons.EdgeCommonsBuilder;
import com.mbreissi.edgecommons.config.ConfigManager;
import com.mbreissi.edgecommons.facades.EventsFacade;
import com.mbreissi.edgecommons.facades.Severity;
import com.mbreissi.edgecommons.heartbeat.InstanceConnectivity;
import com.mbreissi.edgecommons.messaging.Message;
import com.mbreissi.edgecommons.messaging.MessageBuilder;
import com.mbreissi.edgecommons.messaging.MessageIdentity;
import com.mbreissi.edgecommons.messaging.MessagingClient;
import com.mbreissi.edgecommons.messaging.Qos;
import com.mbreissi.edgecommons.metrics.MetricBuilder;
import com.mbreissi.edgecommons.metrics.MetricEmitter;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ArrayBlockingQueue;
import java.util.concurrent.BlockingQueue;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicLong;

/**
 * <<COMPONENTNAME>> — a processing component.
 *
 * <p>A <b>processor</b> subscribes to messages, transforms them, and forwards the result. This
 * scaffold wires that shape end to end; the transformation itself lives in {@link Processor} /
 * {@link Stages}, which is where your code goes.
 *
 * <pre>{@code
 *   subscribe(filter) ──► bounded queue ──► one worker thread per route ──► publish
 *                                                (Pipeline)                local | northbound
 * }</pre>
 *
 * <p>Each entry of {@code component.instances[]} is <b>one route</b>: topic filters, a pipeline of
 * stages, and a target. Routes are independent — one thread each — so a slow route cannot stall
 * another, and the per-key state inside a stage needs no lock.
 *
 * <h2>Why a processor uses {@code getMessaging()} and not {@code getData()}</h2>
 *
 * <p>Worth reading twice, because it is the mistake this archetype invites. The {@code data()}
 * facade is for a component that <i>produces</i> readings: it mints its own topic from a signal id
 * and imposes the {@code SouthboundSignalUpdate} body. A processor is <b>payload-agnostic</b> — it
 * republishes what it was handed, on a topic its route names. Routing that through {@code data()}
 * would rewrite both the topic and the body, which is exactly what a republisher must not do. So:
 * raw {@code edgeCommons.getMessaging()}, and topics from config.
 *
 * <h2>Two guards that are not optional</h2>
 *
 * <ul>
 *   <li><b>Self-echo.</b> A processor that publishes onto a class it also subscribes to will consume
 *       its own output, reprocess it, republish it, and saturate the device.
 *       {@link #isSelfEcho(Message, String, String)} drops anything carrying our own identity.</li>
 *   <li><b>Identity restamp.</b> What we publish is <i>ours</i>. Without the restamp the fleet cannot
 *       tell who emitted a message — and the self-echo guard downstream cannot work either.</li>
 * </ul>
 */
public final class <<COMPONENTNAME>> {

    private static final Logger LOGGER = LogManager.getLogger(<<COMPONENTNAME>>.class);

    /** The metric this component emits each interval. */
    static final String METRIC_NAME = "processorThroughput";

    /** How often the counters below are flushed as a metric. */
    private static final long METRIC_INTERVAL_MS = 60_000;

    private final EdgeCommons edgeCommons;
    private final ConfigManager config;
    private final MessagingClient messaging;
    private final MetricEmitter metrics;
    private final EventsFacade events;
    private final List<RouteConfig> routes;
    private final Stats stats = new Stats();
    private final List<Thread> workers = new ArrayList<>();
    private final CountDownLatch shutdown = new CountDownLatch(1);

    public static void main(String[] args) {
        new <<COMPONENTNAME>>(args).run();
    }

    <<COMPONENTNAME>>(String[] args) {
        edgeCommons = EdgeCommonsBuilder.create("<<COMPONENTFULLNAME>>").withArgs(args).build();
        config = edgeCommons.getConfigManager();
        messaging = edgeCommons.getMessaging();
        metrics = edgeCommons.getMetrics();
        events = edgeCommons.getEvents();

        config.addConfigChangeListener(() -> {
            LOGGER.info("configuration changed: identity={}", config.getComponentIdentity().getPath());
            return true;
        });

        metrics.defineMetric(MetricBuilder.create(METRIC_NAME)
                .withConfig(config)
                .addMeasure("received", "Count", 60)
                .addMeasure("published", "Count", 60)
                .addMeasure("dropped", "Count", 60)
                .addMeasure("errors", "Count", 60)
                .build());

        // ONE provider, TWO surfaces: the library pushes this sample into the `state` keepalive's
        // instances[] every tick AND returns it from the built-in `status` verb when pulled, so a
        // console that subscribes and a console that asks can never disagree. See
        // instanceConnectivity() below for why a processor reports nothing.
        edgeCommons.setInstanceConnectivityProvider(<<COMPONENTNAME>>::instanceConnectivity);

        routes = parseRoutes(config);
        if (routes.isEmpty()) {
            // A malformed route is skipped with a warning rather than killing the component — but if
            // EVERY route is malformed there is nothing to run, and failing loudly beats idling
            // silently.
            throw new IllegalStateException("no valid routes in component.instances[]");
        }
    }

    /** Parses every route; a malformed one is skipped with a warning, not fatal on its own. */
    private static List<RouteConfig> parseRoutes(ConfigManager config) {
        JsonObject global = config.getGlobalConfig();
        JsonObject defaults = global != null && global.has("defaults")
                ? global.getAsJsonObject("defaults") : null;

        List<RouteConfig> parsed = new ArrayList<>();
        for (String id : config.getInstanceIds()) {
            try {
                parsed.add(RouteConfig.parse(config.getInstanceConfig(id), defaults));
            } catch (RuntimeException e) {
                LOGGER.warn("skipping malformed route `{}`: {}", id, e.getMessage());
            }
        }
        return parsed;
    }

    /** Subscribes every route, starts a worker per route, then flushes metrics until shutdown. */
    void run() {
        MessageIdentity me = config.getComponentIdentity();
        String myPath = me.getPath();
        String myComponent = me.getComponent();

        for (RouteConfig route : routes) {
            BlockingQueue<ProcMsg> queue = new ArrayBlockingQueue<>(route.maxQueue());

            for (String filter : route.subscribe()) {
                messaging.subscribe(filter, (topic, msg) -> {
                    if (Guards.isSelfEcho(msg, myPath, myComponent)) {
                        return; // our own output; consuming it would loop forever
                    }
                    stats.received.incrementAndGet();
                    // offer, never put: a full queue must DROP and be COUNTED, not block the
                    // transport's dispatch thread.
                    if (!queue.offer(new ProcMsg(topic, msg))) {
                        stats.dropped.incrementAndGet();
                    }
                }, 1, route.maxQueue());
                LOGGER.info("route {} subscribed to {}", route.id(), filter);
            }

            Thread worker = new Thread(() -> runRoute(route, queue), "route-" + route.id());
            worker.setDaemon(true);
            workers.add(worker);
            worker.start();
        }

        // The library installs its own SIGTERM/SIGINT hook for its subsystems; this one stops OUR
        // route workers and flushes the counters, so a graceful stop does not lose the last window.
        Runtime.getRuntime().addShutdownHook(new Thread(this::stop, "<<COMPONENTNAME>>-stop"));

        try {
            while (!shutdown.await(METRIC_INTERVAL_MS, TimeUnit.MILLISECONDS)) {
                emitMetrics();
            }
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        }
        metrics.flushMetrics();
    }

    /** Stops the route workers. Idempotent — a SIGTERM and an app-driven stop may both arrive. */
    void stop() {
        shutdown.countDown();
        workers.forEach(Thread::interrupt);
    }

    /**
     * One route's worker. Three things happen here, and they are the archetype: a message arrived →
     * run the pipeline; the tick fired → let stateful stages emit; we are stopping → drain the
     * pipeline once more so a half-full window is emitted rather than silently lost.
     */
    private void runRoute(RouteConfig route, BlockingQueue<ProcMsg> queue) {
        Pipeline pipeline = new Pipeline(route.pipeline());
        long nextTick = System.currentTimeMillis() + route.tickMs();

        while (shutdown.getCount() > 0) {
            try {
                long waitMs = Math.max(0, nextTick - System.currentTimeMillis());
                ProcMsg m = queue.poll(waitMs, TimeUnit.MILLISECONDS);
                if (m != null) {
                    dispatch(route, pipeline.run(List.of(m), null));
                }
                long now = System.currentTimeMillis();
                if (now >= nextTick) {
                    dispatch(route, pipeline.run(List.of(), now));
                    nextTick = now + route.tickMs();
                }
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                break;
            }
        }

        // A final tick on the way out.
        dispatch(route, pipeline.run(drain(queue), System.currentTimeMillis()));
        LOGGER.info("route {} stopped", route.id());
    }

    private static List<ProcMsg> drain(BlockingQueue<ProcMsg> queue) {
        List<ProcMsg> remaining = new ArrayList<>();
        queue.drainTo(remaining);
        return remaining;
    }

    /** Restamps identity and publishes each result on the route's target. */
    private void dispatch(RouteConfig route, List<ProcMsg> out) {
        for (ProcMsg m : out) {
            // Restamp identity: what we publish is OURS, not the producer's. .withConfig(...) is the
            // single stamping site.
            Message msg = MessageBuilder.create(m.msg().getHeader().getName(), m.msg().getHeader().getVersion())
                    .withPayload(m.msg().getBody())
                    .withConfig(config)
                    .build();
            try {
                switch (route.target()) {
                    case LOCAL -> messaging.publish(route.publishTopic(), msg);
                    case NORTHBOUND -> messaging.publishNorthbound(route.publishTopic(), msg, Qos.AT_LEAST_ONCE);
                }
                stats.published.incrementAndGet();
            } catch (RuntimeException e) {
                stats.errors.incrementAndGet();
                LOGGER.warn("route {} could not publish to {}: {}", route.id(), route.publishTopic(), e.toString());
                JsonObject context = new JsonObject();
                context.addProperty("route", route.id());
                context.addProperty("topic", route.publishTopic());
                events.emit(Severity.WARNING, "publish-failed",
                        "route " + route.id() + " could not publish", context);
            }
        }
    }

    private void emitMetrics() {
        Map<String, Float> values = new HashMap<>();
        values.put("received", (float) stats.received.getAndSet(0));
        values.put("published", (float) stats.published.getAndSet(0));
        values.put("dropped", (float) stats.dropped.getAndSet(0));
        values.put("errors", (float) stats.errors.getAndSet(0));
        metrics.emitMetric(METRIC_NAME, values);
    }

    /**
     * The per-instance connectivity this component reports — <b>none</b>. A processor's routes are
     * not connections: it consumes off the bus and publishes back onto it, and the bus is the
     * library's business, not an instance of ours. A component with no instances reports none — its
     * {@code state} keepalive carries no {@code instances[]} section, and the built-in
     * {@code status} verb answers exactly as {@code ping} does
     * ({@code {"status":"RUNNING","uptimeSecs":n}}). That is the honest answer, not a gap.
     *
     * <p>If your processor <i>does</i> own a connection (an enrichment database, a model server it
     * calls per message), return one entry per connection instead — each a cached status read, never
     * live IO: this runs on the heartbeat thread every tick.
     *
     * <pre>{@code
     * return List.of(InstanceConnectivity.of("enrichment-db", pool.isUp(), "postgres://…")
     *         .withState("BACKOFF")                                              // OUR vocabulary
     *         .withAttributes(Map.of("lastError", new JsonPrimitive("timeout")))); // domain data
     * }</pre>
     *
     * <p>{@code connected} is the one <b>normalized</b> field and is always present, so any console
     * renders a health dot for any component without knowing that component's vocabulary.
     * {@code state} is our <i>own</i> token for what a boolean cannot say ("reconnecting" vs
     * "administratively disabled"), and {@code attributes} is an open bag: domain data goes there,
     * where it can never destabilize the fields every consumer relies on.
     */
    static List<InstanceConnectivity> instanceConnectivity() {
        return List.of();
    }

    /** Counters, reported as a metric each interval. */
    static final class Stats {
        final AtomicLong received = new AtomicLong();
        final AtomicLong published = new AtomicLong();
        /**
         * Dropped because a route's queue was full. <b>Never let this be invisible</b> — a processor
         * that silently discards messages is worse than one that crashes.
         */
        final AtomicLong dropped = new AtomicLong();
        final AtomicLong errors = new AtomicLong();
    }
}

/**
 * The pure, broker-free guards the run loop delegates to — kept in a top-level class so they are
 * unit-tested directly. The component class itself is a live bootstrap + per-route worker loop (it
 * needs a broker and a running {@code EdgeCommons} to do anything) and is validated on real
 * infrastructure, so it is excluded from the in-process coverage gate; the load-bearing decision it
 * makes on every inbound message — <b>the self-echo guard</b> — is not, so it lives here.
 */
final class Guards {

    private Guards() {
    }

    /**
     * Would consuming this message mean consuming our own output? A processor that publishes onto a
     * class it also subscribes to would otherwise reprocess its own output forever and saturate the
     * device — this is the check that stops it, and it is why the identity restamp in {@code dispatch}
     * is load-bearing: the guard has nothing to compare against if a producer's identity is forwarded
     * as if it were our own.
     *
     * @param msg         the inbound message
     * @param myPath      our own UNS hierarchy path
     * @param myComponent our own component token
     * @return {@code true} when the message carries our own device + component identity
     */
    static boolean isSelfEcho(Message msg, String myPath, String myComponent) {
        MessageIdentity id = msg.getIdentity();
        return id != null
                && myPath.equals(id.getPath())
                && myComponent.equals(id.getComponent());
    }
}
