/**
 * <<COMPONENTNAME>> — application logic.
 *
 * Minimal starting point: holds the `ggcommons` service handles, registers a
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
 * on the console's Events/Metrics tabs and something custom to command, instead of an empty
 * dashboard:
 * - a periodic **metric** ({@link METRIC_NAME}: a monotonic `tickCount` counter plus an
 *   `uptimeSecs` gauge-like measure) via `gg.metrics()`;
 * - a periodic **evt** (`ecv1/.../evt/sample-event`) via the UNS topic builder + `MessageBuilder`
 *   — there is no dedicated `events()` facade yet, so an evt is just a normal published message
 *   on the open `evt` class;
 * - a custom **command verb** ({@link SET_GREETING}), registered with
 *   `gg.commands().register(...)` alongside the automatic built-ins, that mutates a small piece
 *   of in-memory state which the periodic status publish then reflects on its very next tick —
 *   so invoking it from the console is visibly observable.
 *
 * Replace all three with your own business metrics/events/verbs; none of this is required by the
 * library (a bare scaffold works fine without them), it exists so the demonstrated surface is
 * live end-to-end out of the box.
 */
import {
  CommandException,
  Config,
  ConfigurationChangeListener,
  GGCommons,
  IMessagingService,
  Message,
  MessageBuilder,
  MetricBuilder,
  MetricService,
  Uns,
  UnsClass,
  logger,
} from "@edgecommons/ggcommons";

/** The demo loop-tick metric name (see the module docs). */
const METRIC_NAME = "loopTicks";
/** The custom command verb this scaffold registers (see the module docs). */
const SET_GREETING = "set-greeting";
/** How often the demo loop ticks (publishes the status/evt/metric trio below), in ms. */
const TICK_INTERVAL_MS = 10_000;

const sleep = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

/** The component's business logic and the `ggcommons` service handles it operates over. */
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
   * In-memory demo state: mutated by the {@link SET_GREETING} command (registered below), read
   * back by the periodic status publish in {@link run} — so a console "Send command" has a
   * visible effect without needing a dedicated custom "get" verb.
   */
  private greeting = "Hello from <<COMPONENTNAME>>";
  /** Flipped by {@link stop} to end the tick loop in {@link run}. */
  private stopped = false;

  constructor(gg: GGCommons) {
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
    // APP is the free application class; EVT is for discrete, notable occurrences (this
    // scaffold's sample event) — metric publishes go through `this.metrics` above, never a
    // hand-built topic.
    const statusTopic = this.uns.topic(UnsClass.App, "status");
    const eventTopic = this.uns.topic(UnsClass.Evt, "sample-event");
    logger.info(
      `UNS identity path: ${this.uns.identity().path} - status=${statusTopic} event=${eventTopic}`,
    );

    const start = Date.now();
    let seq = 0;
    while (!this.stopped) {
      seq += 1;
      const uptimeSecs = Math.floor((Date.now() - start) / 1000);
      await this.tick(seq, uptimeSecs, statusTopic, eventTopic);
      await sleep(TICK_INTERVAL_MS);
    }
  }

  /** One demo tick: the status/metric/evt trio (see the class docs). */
  private async tick(seq: number, uptimeSecs: number, statusTopic: string, eventTopic: string): Promise<void> {
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

      // 3) evt - a discrete, human-meaningful occurrence (not a metric, not liveness state);
      // the console's Events tab. A real component would emit these on actual occurrences (a
      // threshold crossed, a connection lost/restored, ...), not on a fixed timer.
      const eventMsg = MessageBuilder.create("SampleEvent", "1.0")
        .withPayload({
          severity: "info",
          message: "sample event from <<COMPONENTNAME>>",
          context: { seq, greeting: this.greeting },
        })
        .withConfig(this.config)
        .build();
      await this.messaging
        .publish(eventTopic, eventMsg)
        .catch((e: unknown) => logger.warn(`event publish failed: ${String(e)}`));
    }

    // 2) metric - a loop-tick counter plus an uptime-ish gauge (the console's Metrics tab).
    await this.metrics
      .emitMetric(METRIC_NAME, { tickCount: seq, uptimeSecs })
      .catch((e: unknown) => logger.warn(`metric emit failed: ${String(e)}`));

    logger.info(`tick seq=${seq} uptimeSecs=${uptimeSecs} greeting=${this.greeting}`);
  }

  /** Stop the app and clean up before the runtime is closed. */
  async stop(): Promise<void> {
    this.stopped = true;
  }
}
