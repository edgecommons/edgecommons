/**
 * <<COMPONENTNAME>> — the runtime seam.
 *
 * This is the **thin live-runtime seam**: it wires the `edgecommons` service handles together and
 * drives the demo loop. It needs a live runtime (a built {@link EdgeCommons}, a messaging transport,
 * an interval clock) to do anything, so it is validated by the HOST / GREENGRASS / KUBERNETES deploy
 * paths (the validation matrix), not by a unit test — and it is excluded from the coverage
 * denominator in `vitest.config.ts`. The decisions worth unit-testing (the command verb, the
 * connectivity provider) live in `src/app.ts`, which this module orchestrates; keep this file free
 * of logic a test could exercise, so the exclusion stays honest.
 */
import {
  Config,
  ConfigurationChangeListener,
  DataFacade,
  EdgeCommons,
  EventsFacade,
  IMessagingService,
  Message,
  MessageBuilder,
  MetricBuilder,
  MetricService,
  Severity,
  Uns,
  UnsClass,
  logger,
} from "@edgecommons/edgecommons";

import {
  DATA_SIGNAL_ID,
  INITIAL_GREETING,
  METRIC_NAME,
  SET_GREETING,
  TICK_INTERVAL_MS,
  applyGreeting,
  instanceConnectivity,
} from "./app";

const sleep = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

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
  private greeting = INITIAL_GREETING;
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
    // See instanceConnectivity() in src/app.ts for what this scaffold reports, and why.
    gg.setInstanceConnectivityProvider(instanceConnectivity);

    // --- commands: ping/reload-config/get-configuration are already live (wired by the library
    // before this constructor runs). Register ONE custom verb so there is something for the
    // console's "Send command" to invoke beyond the built-ins. commands() is undefined only
    // when no messaging is available in this runtime mode. The verb's decision is applyGreeting()
    // (src/app.ts, unit-tested); here we only apply its result to our in-memory state.
    const commands = gg.commands();
    if (commands) {
      commands.register(SET_GREETING, (request: Message) => {
        const change = applyGreeting(request.body, this.greeting);
        this.greeting = change.greeting;
        return change;
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

  /** One demo tick: the status/metric/data/evt quartet (see src/app.ts's module docs). */
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
