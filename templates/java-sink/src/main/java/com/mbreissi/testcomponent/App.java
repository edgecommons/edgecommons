package <<PACKAGE>>;

import com.google.gson.Gson;
import com.google.gson.JsonObject;
import com.google.gson.JsonPrimitive;
import com.mbreissi.edgecommons.EdgeCommons;
import com.mbreissi.edgecommons.EdgeCommonsBuilder;
import com.mbreissi.edgecommons.config.ConfigManager;
import com.mbreissi.edgecommons.facades.EventsFacade;
import com.mbreissi.edgecommons.facades.Severity;
import com.mbreissi.edgecommons.heartbeat.InstanceConnectivity;
import com.mbreissi.edgecommons.messaging.Message;
import com.mbreissi.edgecommons.messaging.MessagingClient;
import com.mbreissi.edgecommons.metrics.MetricBuilder;
import com.mbreissi.edgecommons.metrics.MetricEmitter;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ArrayBlockingQueue;
import java.util.concurrent.BlockingQueue;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ThreadLocalRandom;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicLong;

/**
 * <<COMPONENTNAME>> — a sink component.
 *
 * <p>A <b>sink</b> is the last thing standing between data and its destination. It consumes work,
 * delivers it outward, and only then lets go of the source.
 *
 * <pre>{@code
 *   consume ──► deliver (idempotent, stable key) ──► verify ──► confirm ──► report
 *                        ▲                                                    │
 *                        └────────── retry with full jitter ◄─────────────────┘
 * }</pre>
 *
 * <p>The ordering <b>is</b> the archetype, and every step earns its place:
 *
 * <ul>
 *   <li><b>Deliver idempotently, to a stable key.</b> A redelivery overwrites; it does not
 *       duplicate. A sink that cannot retry without duplicating cannot retry at all.</li>
 *   <li><b>Verify before you confirm.</b> Trusting that {@code deliver} returned and releasing the
 *       source without checking what actually landed is how you end up having deleted the only
 *       copy.</li>
 *   <li><b>Classify the failure.</b> Retrying a permanent error burns the budget; giving up on a
 *       transient one loses data a second attempt would have delivered. See
 *       {@link DeliverException}.</li>
 *   <li><b>Report every transition.</b> A sink that fails quietly is indistinguishable from one that
 *       is idle. Started / completed / failed / exhausted all go out on the UNS event surface, and
 *       each destination's current reachability is reported as an <b>instance</b> of this component
 *       ({@link #connectivity}) — a sink's destinations <i>are</i> its instances. The library pushes
 *       that on every {@code state} keepalive and returns it from the built-in {@code status}
 *       verb.</li>
 * </ul>
 *
 * <h2>Where the work comes from</h2>
 *
 * <p>This scaffold's source is a <b>subscription</b>: it consumes messages off the bus and delivers
 * each one. That is the common case. If your source is a watched directory or a polled API, replace
 * the subscribe call in {@link #run()} — everything downstream of
 * {@link #deliverWithRetry(SinkConfig, Item, Destination)} is unchanged, which is the point of the
 * seam.
 */
public final class <<COMPONENTNAME>> {

    private static final Logger LOGGER = LogManager.getLogger(<<COMPONENTNAME>>.class);

    /** The metric this component emits each interval. */
    static final String METRIC_NAME = "sinkDeliveries";

    private static final long METRIC_INTERVAL_MS = 60_000;
    private static final Gson GSON = new Gson();

    private final EdgeCommons edgeCommons;
    private final ConfigManager config;
    private final MessagingClient messaging;
    private final MetricEmitter metrics;
    private final EventsFacade events;
    private final List<SinkConfig> sinks;
    private final Stats stats = new Stats();
    private final List<Thread> workers = new ArrayList<>();
    private final CountDownLatch shutdown = new CountDownLatch(1);

    /**
     * The live reachability of every configured destination, keyed by sink id — <b>a sink's
     * destinations are its instances</b>. Written by the sink workers as deliveries succeed and
     * fail, read on the heartbeat thread; hence concurrent, and hence a cached value rather than a
     * live probe (see {@link #reportConnectivity}).
     */
    private final Map<String, InstanceConnectivity> destinationHealth = new ConcurrentHashMap<>();

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
                .addMeasure("delivered", "Count", 60)
                .addMeasure("retried", "Count", 60)
                .addMeasure("exhausted", "Count", 60)
                .addMeasure("dropped", "Count", 60)
                .build());

        // ONE provider, TWO surfaces: the library pushes this sample into the `state` keepalive's
        // instances[] every tick AND returns it from the built-in `status` verb when pulled — so an
        // operator asking "is the archive still taking writes?" and a console subscribed to state
        // can never disagree. Sampled on the heartbeat thread: it must not block, so it reads the
        // cached map the workers maintain and never touches the destination.
        edgeCommons.setInstanceConnectivityProvider(() -> List.copyOf(destinationHealth.values()));

        sinks = parseSinks(config);
        if (sinks.isEmpty()) {
            throw new IllegalStateException("no valid sinks in component.instances[]");
        }
    }

    private static List<SinkConfig> parseSinks(ConfigManager config) {
        JsonObject global = config.getGlobalConfig();
        JsonObject defaults = global != null && global.has("defaults")
                ? global.getAsJsonObject("defaults") : null;

        List<SinkConfig> parsed = new ArrayList<>();
        for (String id : config.getInstanceIds()) {
            try {
                parsed.add(SinkConfig.parse(config.getInstanceConfig(id), defaults));
            } catch (RuntimeException e) {
                LOGGER.warn("skipping malformed sink `{}`: {}", id, e.getMessage());
            }
        }
        return parsed;
    }

    void run() {
        for (SinkConfig sink : sinks) {
            Destination destination = Destination.build(sink.destination());
            BlockingQueue<Item> queue = new ArrayBlockingQueue<>(sink.maxQueue());

            // Report the destination BEFORE the first delivery: a configured sink that has had no
            // work yet must still appear, or an operator cannot tell "idle" from "not configured".
            // Nothing has failed, so it is reported reachable — replace this with a cheap probe
            // (a HEAD, a bucket stat) if your backend offers one.
            reportConnectivity(sink, destination, true, "IDLE", null);

            messaging.subscribe(sink.subscribe(), (topic, msg) -> {
                stats.received.incrementAndGet();
                Item item = new Item(
                        // A stable, deterministic key: the same message always lands in the same
                        // place, so a redelivery overwrites.
                        keyFor(sink.id(), topic, msg),
                        GSON.toJson(msg.getBody()).getBytes(StandardCharsets.UTF_8));
                // offer, never put: a full queue must DROP and be COUNTED, not block the transport's
                // dispatch thread.
                if (!queue.offer(item)) {
                    stats.dropped.incrementAndGet();
                }
            }, 1, sink.maxQueue());
            LOGGER.info("sink {} subscribed to {} -> {}", sink.id(), sink.subscribe(), destination.kind());

            Thread worker = new Thread(() -> runSink(sink, queue, destination), "sink-" + sink.id());
            worker.setDaemon(true);
            workers.add(worker);
            worker.start();
        }

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

    /** Stops the sink workers. Idempotent — a SIGTERM and an app-driven stop may both arrive. */
    void stop() {
        shutdown.countDown();
        workers.forEach(Thread::interrupt);
    }

    private void runSink(SinkConfig sink, BlockingQueue<Item> queue, Destination destination) {
        while (shutdown.getCount() > 0) {
            try {
                Item item = queue.poll(1, TimeUnit.SECONDS);
                if (item != null) {
                    deliverWithRetry(sink, item, destination);
                }
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                break;
            }
        }
        LOGGER.info("sink {} stopped", sink.id());
    }

    /**
     * Delivers one item, retrying transient failures until the time budget is spent.
     *
     * <p>The event ladder is the sink's contract with whoever is watching: <b>delivery-started</b>,
     * then either <b>delivery-completed</b>, or <b>delivery-failed</b> (carrying {@code willRetry}),
     * and finally <b>delivery-exhausted</b> if the budget runs out. An operator must be able to tell
     * "still trying" from "gave up", and gave-up must be loud — it is Critical, because it is data
     * that did not arrive.
     *
     * @param sink        the sink whose policy governs the retries
     * @param item        the item to deliver
     * @param destination where it goes
     * @return {@code true} when the item was delivered <b>and verified</b>
     */
    boolean deliverWithRetry(SinkConfig sink, Item item, Destination destination) {
        long started = System.nanoTime();
        int attempt = 0;

        events.emit(Severity.INFO, "delivery-started", null,
                context(sink.id(), item.key(), c -> c.addProperty("kind", destination.kind())));

        while (true) {
            try {
                // deliver, then VERIFY. Only a verified delivery is a delivery.
                Delivered delivered = destination.deliver(item);
                destination.verify(item, delivered);

                stats.delivered.incrementAndGet();
                // A verified delivery is the only proof a destination is reachable.
                reportConnectivity(sink, destination, true, "ONLINE", null);
                int attempts = attempt + 1;
                events.emit(Severity.INFO, "delivery-completed", null,
                        context(sink.id(), item.key(), c -> {
                            c.addProperty("attempts", attempts);
                            c.addProperty("elapsedMs", elapsedMs(started));
                        }));
                // The source is released HERE — after verification, never before.
                return true;

            } catch (DeliverException e) {
                if (!e.isTransient()) {
                    // Permanent: it will fail identically forever. Retrying is a waste of the budget
                    // and of the log; give up now and say so.
                    stats.exhausted.incrementAndGet();
                    reportConnectivity(sink, destination, false, "FAILED", e.getMessage());
                    LOGGER.error("sink {} permanently failed on {}: {}", sink.id(), item.key(), e.getMessage());
                    events.emit(Severity.CRITICAL, "delivery-exhausted",
                            sink.id() + " will never deliver " + item.key(),
                            context(sink.id(), item.key(), c -> c.addProperty("reason", e.getMessage())));
                    return false;
                }

                if (sink.retry().budgetSpent(elapsedMs(started))) {
                    stats.exhausted.incrementAndGet();
                    reportConnectivity(sink, destination, false, "FAILED", e.getMessage());
                    int attempts = attempt + 1;
                    LOGGER.error("sink {} spent its retry budget on {} after {} attempts",
                            sink.id(), item.key(), attempts);
                    events.emit(Severity.CRITICAL, "delivery-exhausted",
                            sink.id() + " gave up on " + item.key(),
                            context(sink.id(), item.key(), c -> {
                                c.addProperty("attempts", attempts);
                                c.addProperty("reason", e.getMessage());
                            }));
                    return false;
                }

                long backoff = sink.retry().delayMs(attempt, ThreadLocalRandom.current().nextDouble());
                stats.retried.incrementAndGet();
                // Transient: still trying. `connected=false` says "not delivering"; only OUR `state`
                // can say whether that is a retry in flight or a destination that is gone for good.
                reportConnectivity(sink, destination, false, "BACKOFF", e.getMessage());
                int attempts = attempt + 1;
                LOGGER.warn("sink {} transient failure on {} (attempt {}); retrying in {} ms: {}",
                        sink.id(), item.key(), attempts, backoff, e.getMessage());
                events.emit(Severity.WARNING, "delivery-failed", null,
                        context(sink.id(), item.key(), c -> {
                            c.addProperty("attempt", attempts);
                            c.addProperty("willRetry", true);
                            c.addProperty("nextAttemptInMs", backoff);
                        }));

                try {
                    Thread.sleep(backoff);
                } catch (InterruptedException interrupted) {
                    Thread.currentThread().interrupt();
                    return false;
                }
                attempt++;
            }
        }
    }

    /** Records one destination's reachability for both connectivity surfaces (state + status). */
    private void reportConnectivity(SinkConfig sink, Destination destination, boolean reachable,
                                    String state, String detail) {
        destinationHealth.put(sink.id(), connectivity(sink.id(), destination.kind(), reachable, state, detail));
    }

    /**
     * One destination's connectivity entry.
     *
     * <p>{@code connected} is the one <b>normalized</b> field and is always present, so any console
     * can render a health dot for any sink without knowing what a bucket is. {@code state} is this
     * component's <i>own</i> vocabulary for what a boolean cannot say — {@code BACKOFF} (retrying,
     * the data is still in hand) and {@code FAILED} (gave up, the data did not arrive) are both
     * "not connected", and an operator must be able to tell them apart. {@code attributes} is an
     * open bag for what only a sink knows (the backend kind here; add a bucket, a region, an error
     * code): deliberately unconstrained, so domain data can never destabilize the fields above that
     * every consumer relies on.
     *
     * @param sinkId    the sink id — the instance this entry reports
     * @param kind      the destination kind, as named in config ({@code local}, {@code s3}, …)
     * @param reachable the normalized flag: is this destination taking writes?
     * @param state     our own condition token ({@code IDLE}/{@code ONLINE}/{@code BACKOFF}/{@code FAILED})
     * @param detail    why it is not, or {@code null}
     * @return the entry the {@code state} keepalive and the {@code status} verb both report
     */
    static InstanceConnectivity connectivity(String sinkId, String kind, boolean reachable,
                                             String state, String detail) {
        return InstanceConnectivity.of(sinkId, reachable, detail)
                .withState(state)
                .withAttributes(Map.of("kind", new JsonPrimitive(kind)));
    }

    private static long elapsedMs(long startedNanos) {
        return (System.nanoTime() - startedNanos) / 1_000_000;
    }

    private static JsonObject context(String sinkId, String key, java.util.function.Consumer<JsonObject> extra) {
        JsonObject c = new JsonObject();
        c.addProperty("sink", sinkId);
        c.addProperty("key", key);
        extra.accept(c);
        return c;
    }

    private void emitMetrics() {
        Map<String, Float> values = new HashMap<>();
        values.put("received", (float) stats.received.getAndSet(0));
        values.put("delivered", (float) stats.delivered.getAndSet(0));
        values.put("retried", (float) stats.retried.getAndSet(0));
        values.put("exhausted", (float) stats.exhausted.getAndSet(0));
        values.put("dropped", (float) stats.dropped.getAndSet(0));
        metrics.emitMetric(METRIC_NAME, values);
    }

    /**
     * A stable, deterministic key for a message.
     *
     * <p>Deterministic is the whole point: the same message must always resolve to the same key, or a
     * retry duplicates instead of overwriting.
     *
     * @param sinkId the sink id — it prefixes the key, so two sinks never collide
     * @param topic  the topic the message arrived on; its last segment groups the objects
     * @param msg    the message — its envelope uuid makes the key unique <i>and</i> reproducible
     * @return the destination key
     */
    static String keyFor(String sinkId, String topic, Message msg) {
        int slash = topic.lastIndexOf('/');
        String leaf = slash >= 0 && slash + 1 < topic.length() ? topic.substring(slash + 1) : "message";
        return sinkId + "/" + leaf + "/" + msg.getHeader().getUuid() + ".json";
    }

    /** Counters, reported as a metric each interval. */
    static final class Stats {
        final AtomicLong received = new AtomicLong();
        final AtomicLong delivered = new AtomicLong();
        final AtomicLong retried = new AtomicLong();
        /** Gave up. This is the number that matters: it is data that did not arrive. */
        final AtomicLong exhausted = new AtomicLong();
        /** Dropped because a sink's queue was full — never let this be invisible. */
        final AtomicLong dropped = new AtomicLong();
    }
}
