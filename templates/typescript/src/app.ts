/**
 * <<COMPONENTNAME>> — application logic.
 *
 * Minimal starting point: holds the `edgecommons` service handles, registers a
 * configuration-change listener (dynamic config pickup), and runs until shutdown.
 *
 * The `state` heartbeat keepalive AND the component command inbox are both **automatic**
 * (library-owned, no code here): the `state` keepalive publishes on
 * `ecv1/{device}/{component}/main/state` (on / 5 s / local by default), and the inbox
 * (`ecv1/{device}/{component}/main/cmd/#`, `gg.commands()`) already answers `ping` /
 * `reload-config` / `get-configuration` before the constructor below even runs.
 *
 * What this scaffold adds is the rest of the monitoring + command surface the edge-console
 * reads (DESIGN-uns §7/§9 — G-S1/S2), so a freshly generated component has something to show up
 * on the console's Signals/Events/Metrics tabs and something custom to command, instead of an
 * empty dashboard:
 * - a periodic **metric** ({@link METRIC_NAME}: a monotonic `tickCount` counter plus an
 *   `uptimeSecs` gauge-like measure) via `gg.metrics()`;
 * - a periodic **data** signal ({@link DATA_SIGNAL_ID}: a sine-wave demo reading) via
 *   `gg.data()` — the {@link DataFacade} constructs the `SouthboundSignalUpdate` body
 *   (device/signal/samples) and defaults an omitted sample quality to `GOOD`, so the console's
 *   Signals tab has something to chart;
 * - a periodic **evt** (`ecv1/.../evt/info/sample-event`) via `gg.events()` — the
 *   {@link EventsFacade} derives the `evt/{severity}/{type}` channel from the body's own
 *   severity + type, so the topic and body can never disagree;
 * - a custom **command verb** ({@link SET_GREETING}), registered with
 *   `gg.commands().register(...)` alongside the automatic built-ins, that mutates a small piece
 *   of in-memory state which the periodic status publish then reflects on its very next tick —
 *   so invoking it from the console is visibly observable;
 * - an **instance-connectivity provider** ({@link instanceConnectivity}) — the one source both the
 *   `state` keepalive (push) and the built-in `status` verb (pull) read. This scaffold owns no
 *   connections, so it reports none; the function's docs show where a component that does adds them.
 *
 * Replace all four with your own business metrics/signals/events/verbs; none of this is required
 * by the library (a bare scaffold works fine without them), it exists so the demonstrated surface
 * is live end-to-end out of the box.
 */
import {
  CommandException,
  Config,
  ConfigurationChangeListener,
  DataFacade,
  EventsFacade,
  EdgeCommons,
  IMessagingService,
  InstanceConnectivity,
  Message,
  MessageBuilder,
  MetricBuilder,
  MetricService,
  Severity,
  Uns,
  UnsClass,
  logger,
} from "@edgecommons/edgecommons";

/** The demo loop-tick metric name (see the module docs). */
const METRIC_NAME = "loopTicks";
/** The demo data() signal id (see the module docs). */
const DATA_SIGNAL_ID = "demo-signal";
/** The custom command verb this scaffold registers (see the module docs). */
const SET_GREETING = "set-greeting";
/** How often the demo loop ticks (publishes the status/metric/data/evt quartet below), in ms. */
const TICK_INTERVAL_MS = 10_000;

const sleep = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

/**
 * The per-instance connectivity this component reports — **none**.
 *
 * A component with no southbound connections has no instances to report, and reporting none is the
 * honest answer rather than a gap: the `state` keepalive then carries no `instances[]` section, and
 * the built-in `status` verb answers exactly as `ping` does (`{"status":"RUNNING","uptimeSecs":n}`).
 *
 * If this component grows a connection of its own (a device, a database, an upstream API), return
 * one entry per connection instead — each a **cached** status read, never live IO: the provider is
 * sampled on the keepalive interval, and on the command path too.
 *
 * ```ts
 * return [
 *   InstanceConnectivity.of("enrichment-db", pool.isUp(), "postgres://…")
 *     .withState("BACKOFF")                          // OUR vocabulary
 *     .withAttributes({ lastError: "timeout" }),     // domain data
 * ];
 * ```
 *
 * `connected` is the one **normalized** field and is always present, so any console renders a health
 * dot for any component without knowing that component's vocabulary. `state` is our *own* token for
 * what a boolean cannot say ("reconnecting" vs "administratively disabled"), and `attributes` is an
 * open bag: domain data goes there, where it can never destabilize the fields every consumer reads.
 */
export function instanceConnectivity(): InstanceConnectivity[] {
  return [];
}

/** The component's business logic and the `edgecommons` service handles it operates over. */
export class App {
  private readonly config: Config;
  private readonly metrics: MetricService;
  /**
   * The UNS topic builder bound to this component's config-resolved identity (the top-level
   * `hierarchy` + `identity` config blocks; the last hierarchy level's value is always the
   * resolved thing name). Mint every topic here — e.g.
   * `this.uns.topic(UnsClass.Data, "my-channel")` →
   * `ecv1/{device}/{component}/main/data/my-channel` — never hand-write topic strings.
   * Instance-scoped topics/messages come from `gg.instance(id).uns()` / `.newMessage(...)`.
   */
  private readonly uns: Uns;
  /** `undefined` when no messaging transport is available for the runtime mode. */
  private readonly messaging?: IMessagingService;
  /**
   * The `data()`/`events()` publish facades (DESIGN-class-facades.md), bound to this
   * component's `main` instance. `undefined` for the same reason {@link messaging} can be —
   * both throw if no transport is wired, so both are guarded with the same try/catch below.
   */
  private readonly data?: DataFacade;
  private readonly events?: EventsFacade;
  /**
   * In-memory demo state: mutated by the {@link SET_GREETING} command (registered below), read
   * back by the periodic status publish in {@link run} — so a console "Send command" has a
   * visible effect without needing a dedicated custom "get" verb.
   */
  private greeting = "Hello from <<COMPONENTNAME>>";
  /** Flipped by {@link stop} to end the tick loop in {@link run}. */
  private stopped = false;

  constructor(gg: EdgeCommons) {
    this.config = gg.config();
    this.metrics = gg.metrics();
    this.uns = gg.uns();

    // Dynamic config pickup: react to deployment/shadow config changes at runtime.
    const listener: ConfigurationChangeListener = {
      onConfigurationChange: (config: Config): boolean => {
        logger.info(`configuration changed (thing=${config.thingName})`);
        return true;
      },
    };
    gg.addConfigChangeListener(listener);

    // gg.messaging() throws if no transport is wired (e.g. GREENGRASS without IPC),
    // so guard with try/catch and degrade to heartbeat-only when unavailable.
    try {
      this.messaging = gg.messaging();
    } catch {
      this.messaging = undefined;
    }

    // gg.data()/gg.events() throw for the exact same reason (no transport wired) — same guard,
    // same degrade-to-undefined; tick() below only publishes through them when defined.
    try {
      this.data = gg.data();
    } catch {
      this.data = undefined;
    }
    try {
      this.events = gg.events();
    } catch {
      this.events = undefined;
    }

    // --- metrics: define once, emit every tick in run(). MetricBuilder is the sanctioned
    // construction path (never construct Metric directly). Two measures show a metric isn't
    // just a single scalar: a monotonic counter (tickCount) and a gauge-like elapsed value
    // (uptimeSecs); addDimension adds a custom EMF/CloudWatch dimension on top of the library's
    // own default coreName/component dimensions.
    this.metrics.defineMetric(
      MetricBuilder.create(METRIC_NAME)
        .withConfig(this.config)
        .addMeasure("tickCount", "Count", 60)
        .addMeasure("uptimeSecs", "Seconds", 60)
        .addDimension("demo", "scaffold")
        .build(),
    );

    // --- instance connectivity: ONE provider, TWO surfaces. Whatever it returns is pushed into
    // the `state` keepalive's `instances[]` on every tick AND returned by the built-in `status`
    // verb when a console asks — whoever watches and whoever asks cannot get different answers.
    // See instanceConnectivity() above for what this scaffold reports, and why.
    gg.setInstanceConnectivityProvider(instanceConnectivity);

    // --- commands: ping/reload-config/get-configuration are already live (wired by the library
    // before this constructor runs). Register ONE custom verb so there is something for the
    // console's "Send command" to invoke beyond the built-ins. commands() is undefined only
    // when no messaging is available in this runtime mode.
    const commands = gg.commands();
    if (commands) {
      commands.register(SET_GREETING, (request: Message) => {
        const body = request.body;
        const next =
          typeof body === "object" && body !== null && typeof (body as Record<string, unknown>).greeting === "string"
            ? ((body as Record<string, unknown>).greeting as string)
            : undefined;
        if (next === undefined) {
          throw new CommandException("BAD_ARGS", 'expected a JSON body {"greeting": "<text>"}');
        }
        const previous = this.greeting;
        this.greeting = next;
        return { previousGreeting: previous, greeting: next };
      });
    }
  }

  /**
   * Run until {@link stop} flips the loop flag or the process receives a termination signal —
   * SIGTERM/SIGINT is handled entirely by the library's own installer (FR-HB-2, see main.ts's
   * top comment): it calls `gg.close()` and exits the process directly, independent of this
   * loop, mirroring the Java/Python/Rust scaffolds' own infinite loops (there is no explicit
   * exit condition here under normal operation).
   */
  async run(): Promise<void> {
    logger.info(`<<COMPONENTNAME>> running (thing=${this.config.thingName})`);

    // Publish on unified-namespace (UNS) topics minted via `this.uns` — never hand-write topics.
    // APP is the free application class for this scaffold's status publish; the data()/events()
    // facades below mint their OWN topics from the signal id / severity+type.
    const statusTopic = this.uns.topic(UnsClass.App, "status");
    logger.info(`UNS identity path: ${this.uns.identity().path} - status=${statusTopic}`);

    const start = Date.now();
    let seq = 0;
    while (!this.stopped) {
      seq += 1;
      const uptimeSecs = Math.floor((Date.now() - start) / 1000);
      await this.tick(seq, uptimeSecs, statusTopic);
      await sleep(TICK_INTERVAL_MS);
    }
  }

  /** One demo tick: the status/metric/data/evt quartet (see the class docs). */
  private async tick(seq: number, uptimeSecs: number, statusTopic: string): Promise<void> {
    if (this.messaging) {
      // 1) app status - reflects the current greeting (mutable via the set-greeting command
      // above), so a console operator can watch a command's effect land on the next tick.
      const statusMsg = MessageBuilder.create("StatusUpdate", "1.0")
        .withPayload({ seq, message: this.greeting })
        .withConfig(this.config)
        .build();
      await this.messaging
        .publish(statusTopic, statusMsg)
        .catch((e: unknown) => logger.warn(`status publish failed: ${String(e)}`));
    }

    // 2) metric - a loop-tick counter plus an uptime-ish gauge (the console's Metrics tab).
    await this.metrics
      .emitMetric(METRIC_NAME, { tickCount: seq, uptimeSecs })
      .catch((e: unknown) => logger.warn(`metric emit failed: ${String(e)}`));

    // 3) data - a periodic sample telemetry signal (the console's Signals tab), through the
    // data() facade: it constructs the SouthboundSignalUpdate body (device/signal/samples),
    // sanitizes the channel, and stamps identity - a real adapter maps one protocol read onto
    // addSample(...) and never touches the envelope or topic (DESIGN-class-facades §2.1). A
    // sine wave stands in for a live sensor reading here; publish(path, value) with no explicit
    // Quality demonstrates the facade's honest default - an unspecified reading defaults to
    // Quality.Good (marked qualityRaw="unspecified" on the wire so a consumer can tell a
    // synthesized GOOD from a device-reported one). Pass an explicit Quality when your source
    // knows a read failed or is stale.
    if (this.data) {
      const demoValue = 20.0 + 5.0 * Math.sin(seq / 10.0);
      await this.data
        .publish(DATA_SIGNAL_ID, demoValue)
        .catch((e: unknown) => logger.warn(`data publish failed: ${String(e)}`));
    }

    // 4) evt - a discrete, human-meaningful occurrence (not a metric, not liveness state); the
    // console's Events tab. Through the events() facade: emit(severity, type, message, context)
    // derives the evt/{severity}/{type} channel from the body's own severity + type, so the
    // topic and body can never disagree (DESIGN-class-facades §2.2) - no more hand-built
    // body/topic. A real component would emit these on actual occurrences (a threshold crossed,
    // a connection lost/restored, ...), not on a fixed timer; raiseAlarm/clearAlarm are there
    // for stateful alarms.
    if (this.events) {
      await this.events
        .emit(Severity.Info, "sample-event", "sample event from <<COMPONENTNAME>>", {
          seq,
          greeting: this.greeting,
        })
        .catch((e: unknown) => logger.warn(`event publish failed: ${String(e)}`));
    }

    logger.info(`tick seq=${seq} uptimeSecs=${uptimeSecs} greeting=${this.greeting}`);
  }

  /** Stop the app and clean up before the runtime is closed. */
  async stop(): Promise<void> {
    this.stopped = true;
  }
}
