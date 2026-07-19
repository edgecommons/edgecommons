/**
 * # <<COMPONENTNAME>> — a southbound protocol adapter
 *
 * An **adapter** connects to devices, reads signals, and publishes them onto the UNS in the shape
 * the rest of the fleet expects — so that a consumer can chart a Modbus register and an OPC UA node
 * without knowing either protocol.
 *
 * ```text
 *   connect ──► poll ──► publish SouthboundSignalUpdate ──► report health
 *      ▲                                                         │
 *      └──────────── reconnect with backoff ◄────────────────────┘
 * ```
 *
 * One loop per instance: an instance is one device, and its connection lifecycle is its own. That
 * loop also owns a **control channel** ({@link DeviceControl}) — every command that must touch the
 * session or serialize with the poll loop is *sent* to the loop, and *confirmed* through the reply
 * that rides it. The command surface itself lives in {@link module:commands} (`src/commands.ts`).
 *
 * ## The contract you are implementing (docs/SOUTHBOUND.md)
 *
 * * Publish `SouthboundSignalUpdate` on the `data` class, **via the `data()` facade** — never
 *   hand-build the body and never hand-write the topic.
 * * **Quality on every sample**, normalized to `GOOD | BAD | UNCERTAIN`, with the native code in
 *   `qualityRaw`.
 * * Emit **`southbound_health`** (the exact §5 set — see `src/metrics.ts`), dimensioned by
 *   instance, so an operator can see a link go down without reading logs.
 * * Report **per-instance connectivity** ({@link connectivityOf}).
 * * Serve **read/write/browse/reconnect/pause commands** — and allow-list the writes.
 */
import {
  Config,
  ConfigurationChangeListener,
  DataFacade,
  EdgeCommons,
  EventsFacade,
  InstanceConnectivity,
  MetricService,
  Quality as LibQuality,
  Severity,
  logger,
} from "@edgecommons/edgecommons";

import {
  BrowseError,
  BrowsePage,
  ConnectionConfig,
  DeviceBackend,
  DeviceError,
  DeviceSession,
  Quality,
  Reading,
  SignalInfo,
  backendFor,
} from "./device";
import { DeviceMetrics } from "./metrics";
// `commands.ts` imports only TYPES from this module (erased at compile), so this value import of
// `registerAll` creates no runtime import cycle. `DeviceHandle` is a type-only import.
import { DeviceHandle, registerAll } from "./commands";

/** How often the periodic metrics emit runs, in the poll loop (ms). */
const METRICS_INTERVAL_MS = 30_000;
/** The `component.global.healthThresholds.staleSignalSecs` default (SOUTHBOUND.md §4/§5). */
const DEFAULT_STALE_SIGNAL_SECS = 30;
const DEFAULT_POLL_MS = 5_000;

// --- config ----------------------------------------------------------------------------------

/**
 * Writes are **allow-listed by stable `signal.id`**. An empty list means this adapter is
 * read-only, which is the correct default for anything touching a control system.
 */
export class Writes {
  constructor(readonly allow: readonly string[] = []) {}

  permits(signalId: string): boolean {
    return this.allow.includes(signalId);
  }
}

/** One device == one entry of `component.instances[]`. */
export interface DeviceConfig {
  /**
   * The instance id. It is the `{instance}` token of this device's UNS topics, so it must be a
   * valid UNS token (lower-kebab).
   */
  readonly id: string;
  /** Which backend to use. Matches {@link DeviceBackend.kind}. */
  readonly adapter: string;
  readonly connection: ConnectionConfig;
  /** How often to read, in milliseconds. */
  readonly pollIntervalMs: number;
  readonly writes: Writes;
}

const DEVICE_KEYS = new Set(["id", "adapter", "connection", "pollIntervalMs", "writes"]);

/**
 * Parse one entry of `component.instances[]`.
 *
 * Unknown keys are rejected rather than ignored: a typo'd key is a mistake, not a no-op. (The
 * `connection` object is the one exception — see {@link ConnectionConfig}.)
 *
 * @throws Error when the entry is malformed
 */
export function parseDevice(raw: unknown): DeviceConfig {
  if (typeof raw !== "object" || raw === null) throw new Error("a device must be an object");
  const o = raw as Record<string, unknown>;

  for (const key of Object.keys(o)) {
    if (!DEVICE_KEYS.has(key)) throw new Error(`unknown key '${key}'`);
  }
  if (typeof o.id !== "string" || o.id === "") throw new Error("`id` is required");

  const connection = o.connection;
  if (typeof connection !== "object" || connection === null) {
    throw new Error("`connection` is required");
  }
  const endpoint = (connection as Record<string, unknown>).endpoint;
  if (typeof endpoint !== "string" || endpoint === "") {
    throw new Error("`connection.endpoint` is required");
  }

  const adapter = o.adapter === undefined ? "sim" : o.adapter;
  if (typeof adapter !== "string") throw new Error("`adapter` must be a string");

  const pollIntervalMs = o.pollIntervalMs === undefined ? DEFAULT_POLL_MS : o.pollIntervalMs;
  if (typeof pollIntervalMs !== "number" || pollIntervalMs < 1) {
    throw new Error("`pollIntervalMs` must be a positive number");
  }

  let writes = new Writes();
  if (o.writes !== undefined) {
    if (typeof o.writes !== "object" || o.writes === null) throw new Error("`writes` must be an object");
    const w = o.writes as Record<string, unknown>;
    for (const key of Object.keys(w)) {
      if (key !== "allow") throw new Error(`unknown key 'writes.${key}'`);
    }
    const allow = w.allow ?? [];
    if (!Array.isArray(allow) || allow.some((s) => typeof s !== "string")) {
      throw new Error("`writes.allow` must be an array of signal ids");
    }
    writes = new Writes(allow as string[]);
  }

  return {
    id: o.id,
    adapter,
    connection: connection as ConnectionConfig,
    pollIntervalMs,
    writes,
  };
}

// --- backoff ---------------------------------------------------------------------------------

/**
 * Reconnect backoff: exponential, capped, with **full jitter** — so a site whose PLC reboots does
 * not get every adapter in the plant reconnecting in lockstep on the same second.
 */
export class Backoff {
  constructor(
    readonly baseMs = 1_000,
    readonly maxMs = 60_000,
  ) {}

  /** A random delay in `[0, min(cap, base * 2^attempt))`. `rand01` is injected for the tests. */
  delayMs(attempt: number, rand01: number): number {
    const exp = this.baseMs * 2 ** Math.min(attempt, 20);
    const cap = Math.min(exp, this.maxMs);
    return Math.floor(Math.min(Math.max(rand01, 0), 1) * cap);
  }
}

// --- the device control channel --------------------------------------------------------------

/** The device's answer to a `sb/write` — confirmed, not fire-and-forget. */
export type WriteOutcome = { ok: true } | { ok: false; error: string };
/** The device's answer to a `sb/read`. */
export type ReadOutcome = { ok: true; readings: Reading[] } | { ok: false; error: string };
/** The device's answer to a `sb/browse`. */
export type BrowseOutcome = { ok: true; page: BrowsePage } | { ok: false; error: BrowseError };
/** The device's answer to a `reconnect`. */
export type ReconnectOutcome = { ok: true } | { ok: false; error: string };
/** The device's answer to a `repoll` — the signal count, or a reason it was refused. */
export type RepollOutcome = { ok: true; polled: number } | { ok: false; error: string };

/**
 * One message on a device's **control channel**. Every `sb/*` verb that must touch the session or
 * serialize with the poll loop is delivered as one of these, so the command inbox never touches the
 * session directly — the device's own loop services them one at a time. The reply riding each
 * variant is what makes reads/writes/reconnect *confirmed*.
 */
export type DeviceControl =
  | {
      /** A confirmed, allow-listed write (`sb/write`). The allow-list is checked in the command layer BEFORE this is ever sent. */
      readonly kind: "write";
      readonly signalId: string;
      readonly value: unknown;
      readonly ack: (outcome: WriteOutcome) => void;
    }
  | {
      /** Live-read these ids now (`sb/read`). Serializes with the loop and works while paused. */
      readonly kind: "readNow";
      readonly ids: string[];
      readonly reply: (outcome: ReadOutcome) => void;
    }
  | {
      /** One page of address-space discovery (`sb/browse`). */
      readonly kind: "browse";
      readonly cursor?: string;
      readonly max: number;
      readonly reply: (outcome: BrowseOutcome) => void;
    }
  | {
      /** Pause telemetry production (`sb/pause`). Reply = whether the state changed. */
      readonly kind: "pause";
      readonly reply: (changed: boolean) => void;
    }
  | {
      /** Resume telemetry production (`sb/resume`). Reply = whether the state changed. */
      readonly kind: "resume";
      readonly reply: (changed: boolean) => void;
    }
  | {
      /** Drop + re-establish, one immediate attempt (`reconnect`). */
      readonly kind: "reconnect";
      readonly reply: (outcome: ReconnectOutcome) => void;
    }
  | {
      /** Force an immediate poll now (`repoll`). Refused when paused. */
      readonly kind: "repoll";
      readonly reply: (outcome: RepollOutcome) => void;
    };

/**
 * A single-consumer control mailbox with a deadline.
 *
 * A command cannot touch the session directly: the session lives in the device's own loop, and most
 * device protocols are a single request/response channel that would interleave into nonsense if two
 * callers talked at once. So a command is *sent* to that loop, which serializes it against the
 * reads. `send` returns `false` once the mailbox is {@link close}d (the device loop is gone) — the
 * command layer maps that to `DEVICE_UNAVAILABLE`.
 */
export class Mailbox<T> {
  private readonly queue: T[] = [];
  private waiter?: () => void;
  private closedFlag = false;

  /** Enqueue an item. Returns `false` if the mailbox is closed (the loop is gone). */
  send(item: T): boolean {
    if (this.closedFlag) return false;
    this.queue.push(item);
    this.waiter?.();
    return true;
  }

  /** Take the next item, waiting at most `timeoutMs`. Resolves `undefined` on timeout or close. */
  async receive(timeoutMs: number): Promise<T | undefined> {
    const first = this.queue.shift();
    if (first !== undefined) return first;
    if (this.closedFlag || timeoutMs <= 0) return undefined;

    await new Promise<void>((resolve) => {
      const timer = setTimeout(() => {
        this.waiter = undefined;
        resolve();
      }, timeoutMs);
      this.waiter = () => {
        clearTimeout(timer);
        this.waiter = undefined;
        resolve();
      };
    });
    return this.queue.shift();
  }

  /** Close the mailbox: no further sends, and wake any pending receiver. */
  close(): void {
    this.closedFlag = true;
    this.waiter?.();
  }

  isClosed(): boolean {
    return this.closedFlag;
  }
}

// --- health ----------------------------------------------------------------------------------

/**
 * This adapter's **own vocabulary** for a link's condition — what it reports as
 * `InstanceConnectivity.state`. A boolean cannot tell "still trying" from "backing off after a
 * failure"; an operator needs to, so the richer token rides alongside the normalized flag.
 */
export type LinkState = "CONNECTING" | "ONLINE" | "BACKOFF";

/**
 * The shared per-device state the metrics emitter reads and the connectivity provider renders. The
 * gauges (`connectionState`, latencies) and the interval counters (`readErrors`, `reconnects`) feed
 * `southbound_health` (`src/metrics.ts`); `paused` and `link` feed the connectivity token and
 * `sb/status`. One source, several surfaces — so a health dot, a metric, and a status reply can
 * never disagree.
 */
export class Health {
  /** 1 = connected, 0 = down. */
  connectionState = 0;
  private linkState: LinkState = "CONNECTING";
  /**
   * `true` = telemetry production is paused (`sb/pause`). Read by the connectivity provider and
   * `sb/status`; NOT a `southbound_health` measure (§5 has no `paused`).
   */
  paused = false;
  pollLatencyMs = 0;
  publishLatencyMs = 0;
  /** Reset on each `southbound_health` emit. */
  readErrors = 0;
  /** Reset on each `southbound_health` emit. */
  reconnects = 0;

  /**
   * Record the link's condition. The metric's boolean and the reported state token move
   * **together**, so the health dot and the label a console shows can never disagree.
   */
  setLink(state: LinkState): void {
    this.linkState = state;
    this.connectionState = state === "ONLINE" ? 1 : 0;
  }

  link(): LinkState {
    return this.linkState;
  }

  isPaused(): boolean {
    return this.paused;
  }
}

/**
 * Flip the paused flag, returning whether the state actually changed (idempotent — pausing an
 * already-paused device is not an error).
 */
export function setPaused(health: Health, paused: boolean): boolean {
  const changed = health.paused !== paused;
  health.paused = paused;
  return changed;
}

/**
 * One device's connectivity sample, for the instance-connectivity provider registered in the
 * {@link App} constructor.
 *
 * * `connected` is the **normalized** flag — always present.
 * * `state` is *this adapter's* vocabulary ({@link LinkState}) — `PAUSED` when paused and up, else
 *   the raw link token (so a break while paused still reads `BACKOFF`, `connected` staying truthful).
 * * `attributes` is the **open** bag: domain data only this adapter understands.
 */
export function connectivityOf(cfg: DeviceConfig, health: Health): InstanceConnectivity {
  const link = health.link();
  const connected = link === "ONLINE";
  const paused = health.isPaused();
  const state = paused && connected ? "PAUSED" : link;
  return InstanceConnectivity.of(cfg.id, connected, cfg.connection.endpoint)
    .withState(state)
    .withAttributes({ adapter: cfg.adapter, paused });
}

// --- the southbound publish path ---------------------------------------------------------------

/** Map the backend's protocol-free quality onto the library's wire enum. */
export function toLibQuality(q: Quality): LibQuality {
  switch (q) {
    case Quality.Good:
      return LibQuality.Good;
    case Quality.Bad:
      return LibQuality.Bad;
    case Quality.Uncertain:
      return LibQuality.Uncertain;
  }
}

/**
 * Publish one poll's readings as `SouthboundSignalUpdate`s, returning the count published.
 *
 * The `data()` facade builds the body, mints the topic, and stamps identity. Do not hand-build any
 * of the three. **Every reading is published, including a failed one** — a `BAD` sample says "I
 * could not read this", and silence says nothing at all. When a {@link DeviceMetrics} is passed,
 * each successful publish feeds the staleness tracker.
 */
export async function publishReadings(
  data: DataFacade,
  adapter: string,
  device: Pick<DeviceConfig, "id" | "connection">,
  readings: readonly Reading[],
  dm?: DeviceMetrics,
): Promise<number> {
  let published = 0;
  for (const r of readings) {
    try {
      const signal = data.signal(r.signalId);
      if (r.name !== undefined) signal.name(r.name);
      await signal
        .device(adapter, device.id, device.connection.endpoint)
        .addSample(r.value, { quality: toLibQuality(r.quality), qualityRaw: r.qualityRaw })
        .publish();
      published += 1;
      dm?.onSignalUpdate(r.signalId, Date.now());
    } catch (e) {
      logger.warn(`publish failed instance=${device.id} signal=${r.signalId}: ${String(e)}`);
    }
  }
  return published;
}

// --- the app ---------------------------------------------------------------------------------

const sleep = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));
const rand01 = (): number => Math.random();

/** One poll: read, publish, record latencies + staleness. `polled` = signals published. */
async function pollOnce(
  cfg: DeviceConfig,
  session: DeviceSession,
  data: DataFacade,
  dm: DeviceMetrics,
  health: Health,
): Promise<{ ok: boolean; polled: number }> {
  const started = Date.now();
  let readings: Reading[];
  try {
    readings = await session.readSignals();
  } catch (e) {
    logger.warn(`read failed instance=${cfg.id}; reconnecting: ${String(e)}`);
    health.readErrors += 1;
    return { ok: false, polled: 0 };
  }
  health.pollLatencyMs = Date.now() - started;

  const publishStarted = Date.now();
  const published = await publishReadings(data, cfg.adapter, cfg, readings, dm);
  health.publishLatencyMs = Date.now() - publishStarted;
  return { ok: true, polled: published };
}

/** What ended the poll loop. */
type PollExit =
  | { kind: "linkLost" }
  | { kind: "closed" }
  | { kind: "reconnect"; reply: (outcome: ReconnectOutcome) => void };

/** What servicing the control channel while the session is down concluded. */
type DownOutcome =
  | { kind: "elapsed" }
  | { kind: "closed" }
  | { kind: "reconnect"; reply: (outcome: ReconnectOutcome) => void };

export class App {
  private readonly config: Config;
  private readonly metrics: MetricService;
  private readonly devices: DeviceConfig[] = [];
  /** Each device's control channel: written by the command surface, drained by the device loop. */
  private readonly control = new Map<string, Mailbox<DeviceControl>>();
  /** Each device's health: written by its own loop, read by the connectivity provider. */
  private readonly health = new Map<string, Health>();
  /** Each device's operational-metrics emitter. */
  private readonly deviceMetrics = new Map<string, DeviceMetrics>();
  private readonly staleSignalSecs: number;
  private readonly loops: Promise<void>[] = [];
  private stopped = false;

  constructor(private readonly gg: EdgeCommons) {
    this.config = gg.config();
    this.metrics = gg.metrics();

    const listener: ConfigurationChangeListener = {
      onConfigurationChange: (config: Config): boolean => {
        logger.info(`configuration changed (thing=${config.thingName})`);
        return true;
      },
    };
    gg.addConfigChangeListener(listener);

    this.staleSignalSecs = readStaleSignalSecs(this.config);

    // One device per instance. A malformed device is skipped with a warning rather than killing the
    // component — but if EVERY device is malformed there is nothing to run, and failing loudly
    // beats idling silently.
    for (const id of this.config.instanceIds()) {
      try {
        this.devices.push(parseDevice(this.config.instance(id)));
      } catch (e) {
        logger.warn(`skipping malformed device '${id}': ${String(e)}`);
      }
    }
    if (this.devices.length === 0) {
      throw new Error("no valid devices in component.instances[]");
    }

    // A device's health exists from the moment it is CONFIGURED, not from the moment its loop first
    // connects: a configured device that is down must never be indistinguishable from a device
    // nobody configured. ONE provider, TWO surfaces: the library pushes this sample into the `state`
    // keepalive's `instances[]` every tick, and returns the very same sample from the built-in
    // `status` verb when a console asks. Whoever watches and whoever asks cannot get different answers.
    for (const device of this.devices) {
      this.health.set(device.id, new Health());
    }
    gg.setInstanceConnectivityProvider(() =>
      this.devices.map((d) => connectivityOf(d, this.health.get(d.id) as Health)),
    );
  }

  async run(): Promise<void> {
    // The per-device handles the command surface routes on.
    const handles: DeviceHandle[] = [];

    for (const device of this.devices) {
      // Per-instance facades: data() mints THIS device's topics and stamps its identity.
      const instance = this.gg.instance(device.id);

      const health = this.health.get(device.id) as Health;
      const dm = new DeviceMetrics(this.metrics, this.config, device.id, health, this.staleSignalSecs);
      // Pre-define the metric set so it is fixed and discoverable at startup.
      dm.defineAll();
      this.deviceMetrics.set(device.id, dm);

      // The signal inventory `sb/signals` shows — a config/backend view, no device round-trip.
      const backend = backendFor(device.adapter);
      const signals: SignalInfo[] = backend?.inventory?.(device.connection) ?? [];

      const control = new Mailbox<DeviceControl>();
      this.control.set(device.id, control);

      handles.push({ cfg: device, control, health, dm, signals });

      this.loops.push(
        this.runDevice(device, instance.data(), instance.events(), dm, health, control).catch(
          (e: unknown) => logger.error(`device loop '${device.id}' stopped: ${String(e)}`),
        ),
      );
    }

    // The southbound command surface (`src/commands.ts`). `ping` / `status` / `reload-config` /
    // `get-configuration` are already live — the library registered them before we ran.
    const commands = this.gg.commands();
    if (commands) {
      registerAll(commands, handles);
    }

    await Promise.all(this.loops);
  }

  /**
   * One device's lifecycle: connect, poll, publish, reconnect — and service its control channel.
   *
   * The connect loop and the poll loop are nested on purpose. A read failure that breaks the link
   * drops out of the poll loop and back into connect — the only place that knows how to back off.
   */
  private async runDevice(
    cfg: DeviceConfig,
    data: DataFacade,
    events: EventsFacade,
    dm: DeviceMetrics,
    health: Health,
    control: Mailbox<DeviceControl>,
  ): Promise<void> {
    const backend: DeviceBackend | undefined = backendFor(cfg.adapter);
    if (!backend) {
      logger.error(`unknown adapter '${cfg.adapter}' for instance '${cfg.id}'`);
      return;
    }

    const backoff = new Backoff();
    let attempt = 0;
    // A `reconnect` command's reply, held until the next connect settles it.
    let pendingReconnect: ((outcome: ReconnectOutcome) => void) | undefined;

    while (!this.stopped) {
      // --- CONNECT (servicing control while down, so pause/reconnect don't block on backoff) ---
      let session: DeviceSession | undefined;
      while (!this.stopped && session === undefined) {
        dm.onConnectAttempt();
        health.setLink(attempt === 0 ? "CONNECTING" : "BACKOFF");
        const now = Date.now();
        try {
          session = await backend.connect(cfg.connection);
          attempt = 0;
          dm.onConnected(now);
          health.setLink("ONLINE");
          await dm.emitNow();
          await events
            .emit(Severity.Info, "device-connected", `connected to ${cfg.connection.endpoint}`, {
              instance: cfg.id,
              adapter: backend.kind,
            })
            .catch(() => undefined);
          await events.clearAlarm("device-unreachable", { instance: cfg.id }).catch(() => undefined);
          if (pendingReconnect) {
            pendingReconnect({ ok: true });
            pendingReconnect = undefined;
          }
        } catch (e) {
          dm.onConnectFailure();
          if (pendingReconnect) {
            pendingReconnect({ ok: false, error: String(e) });
            pendingReconnect = undefined;
          }
          // A permanent failure fails identically forever — back off to the ceiling.
          const permanent = !DeviceError.isTransient(e);
          const wait = permanent ? backoff.maxMs : backoff.delayMs(attempt, rand01());
          attempt += 1;
          logger.warn(`connect failed instance=${cfg.id} permanent=${permanent} waitMs=${wait}: ${String(e)}`);
          const outcome = await this.serveWhileDown(control, events, health, wait);
          if (outcome.kind === "reconnect") {
            pendingReconnect = outcome.reply;
            attempt = 0;
          } else if (outcome.kind === "closed") {
            return;
          }
        }
      }
      if (session === undefined) return; // stopped while down

      // --- POLL (until the link breaks or a reconnect is requested) ---
      const exit = await this.runPolling(cfg, session, data, events, dm, health, control);

      // The link is down (or a reconnect asked us to drop it).
      health.setLink("BACKOFF");
      health.reconnects += 1;
      dm.onConnectionDropped(Date.now());
      await dm.emitNow();
      if (!this.stopped) {
        await events
          .raiseAlarm("device-unreachable", `lost the link to ${cfg.connection.endpoint}`, {
            instance: cfg.id,
          })
          .catch(() => undefined);
      }

      if (exit.kind === "closed") return;
      if (exit.kind === "reconnect") pendingReconnect = exit.reply;
      // linkLost: fall through and reconnect.
    }
  }

  /**
   * Read on the poll interval and publish, servicing the control channel, until the link breaks or
   * a reconnect is requested.
   */
  private async runPolling(
    cfg: DeviceConfig,
    session: DeviceSession,
    data: DataFacade,
    events: EventsFacade,
    dm: DeviceMetrics,
    health: Health,
    control: Mailbox<DeviceControl>,
  ): Promise<PollExit> {
    let nextPoll = Date.now() + cfg.pollIntervalMs;
    let nextMetrics = Date.now() + METRICS_INTERVAL_MS;

    while (!this.stopped) {
      const now = Date.now();
      const deadlines = [nextMetrics];
      if (!health.isPaused()) deadlines.push(nextPoll);
      const wait = Math.max(0, Math.min(...deadlines) - now);

      const ctrl = await control.receive(wait);
      if (control.isClosed() && ctrl === undefined) return { kind: "closed" };

      if (ctrl !== undefined) {
        // Poll and control share this one loop, so a write can never race a read on the same
        // connection — most device protocols are a single request/response channel.
        const exit = await this.handleControl(ctrl, cfg, session, data, events, dm, health);
        if (exit !== undefined) return exit;
      } else {
        if (!health.isPaused() && Date.now() >= nextPoll) {
          const r = await pollOnce(cfg, session, data, dm, health);
          if (!r.ok) {
            await session.close().catch(() => undefined);
            return { kind: "linkLost" };
          }
          nextPoll = Date.now() + cfg.pollIntervalMs;
        }
      }

      if (Date.now() >= nextMetrics) {
        await dm.emitPeriodic();
        nextMetrics = Date.now() + METRICS_INTERVAL_MS;
      }
    }
    await session.close().catch(() => undefined);
    return { kind: "closed" };
  }

  /** Service one control message during polling. Returns a {@link PollExit} only for reconnect / link-lost. */
  private async handleControl(
    ctrl: DeviceControl,
    cfg: DeviceConfig,
    session: DeviceSession,
    data: DataFacade,
    events: EventsFacade,
    dm: DeviceMetrics,
    health: Health,
  ): Promise<PollExit | undefined> {
    switch (ctrl.kind) {
      case "write": {
        try {
          await session.writeSignal(ctrl.signalId, ctrl.value);
          ctrl.ack({ ok: true });
        } catch (e) {
          logger.warn(`write failed instance=${cfg.id} signal=${ctrl.signalId}: ${String(e)}`);
          ctrl.ack({ ok: false, error: String(e) });
        }
        return undefined;
      }
      case "readNow": {
        try {
          const readings = await session.readNamed(ctrl.ids);
          ctrl.reply({ ok: true, readings });
        } catch (e) {
          ctrl.reply({ ok: false, error: String(e) });
        }
        return undefined;
      }
      case "browse": {
        try {
          const page = await session.browse(ctrl.cursor, ctrl.max);
          ctrl.reply({ ok: true, page });
        } catch (e) {
          const error = BrowseError.isBrowseError(e) ? e : BrowseError.failed(String(e));
          ctrl.reply({ ok: false, error });
        }
        return undefined;
      }
      case "pause": {
        const changed = setPaused(health, true);
        if (changed) {
          await events
            .emit(Severity.Warning, "adapter-paused", "telemetry production paused", { instance: cfg.id })
            .catch(() => undefined);
        }
        ctrl.reply(changed);
        return undefined;
      }
      case "resume": {
        const changed = setPaused(health, false);
        if (changed) {
          await events
            .emit(Severity.Info, "adapter-resumed", "telemetry production resumed", { instance: cfg.id })
            .catch(() => undefined);
        }
        ctrl.reply(changed);
        return undefined;
      }
      case "reconnect": {
        await session.close().catch(() => undefined);
        return { kind: "reconnect", reply: ctrl.reply };
      }
      case "repoll": {
        if (health.isPaused()) {
          ctrl.reply({ ok: false, error: "instance is paused - resume first" });
          return undefined;
        }
        const r = await pollOnce(cfg, session, data, dm, health);
        if (r.ok) {
          ctrl.reply({ ok: true, polled: r.polled });
          return undefined;
        }
        ctrl.reply({ ok: false, error: "link error" });
        await session.close().catch(() => undefined);
        return { kind: "linkLost" };
      }
    }
  }

  /**
   * Service the control channel while the session is **down**, for up to `waitMs`. Pause/resume
   * take effect (they only need the shared flag + event); the I/O verbs answer "disconnected" (the
   * command layer maps that to `DEVICE_UNAVAILABLE` / `BROWSE_FAILED`); a `reconnect` returns its
   * reply so the caller connects now.
   */
  private async serveWhileDown(
    control: Mailbox<DeviceControl>,
    events: EventsFacade,
    health: Health,
    waitMs: number,
  ): Promise<DownOutcome> {
    const deadline = Date.now() + waitMs;
    for (;;) {
      const remaining = deadline - Date.now();
      if (remaining <= 0) return { kind: "elapsed" };
      const ctrl = await control.receive(remaining);
      if (control.isClosed() && ctrl === undefined) return { kind: "closed" };
      if (ctrl === undefined) return { kind: "elapsed" };

      switch (ctrl.kind) {
        case "reconnect":
          return { kind: "reconnect", reply: ctrl.reply };
        case "pause": {
          const changed = setPaused(health, true);
          if (changed) await events.emit(Severity.Warning, "adapter-paused").catch(() => undefined);
          ctrl.reply(changed);
          break;
        }
        case "resume": {
          const changed = setPaused(health, false);
          if (changed) await events.emit(Severity.Info, "adapter-resumed").catch(() => undefined);
          ctrl.reply(changed);
          break;
        }
        case "write":
          ctrl.ack({ ok: false, error: "device is disconnected" });
          break;
        case "readNow":
          ctrl.reply({ ok: false, error: "device is disconnected" });
          break;
        case "repoll":
          ctrl.reply({ ok: false, error: "device is disconnected" });
          break;
        case "browse":
          ctrl.reply({ ok: false, error: BrowseError.failed("device is disconnected") });
          break;
      }
    }
  }

  /** Stop the device loops and clean up before the runtime is closed. */
  async stop(): Promise<void> {
    this.stopped = true;
    for (const control of this.control.values()) control.close();
    await Promise.allSettled(this.loops);
    await this.metrics.flushMetrics().catch(() => undefined);
  }
}

/** Read `component.global.healthThresholds.staleSignalSecs` (default 30). */
function readStaleSignalSecs(config: Config): number {
  try {
    const global = config.global();
    if (global !== null && typeof global === "object") {
      const thresholds = (global as Record<string, unknown>).healthThresholds;
      if (thresholds !== null && typeof thresholds === "object") {
        const secs = (thresholds as Record<string, unknown>).staleSignalSecs;
        if (typeof secs === "number" && secs >= 1) return secs;
      }
    }
  } catch {
    // fall through to the default
  }
  return DEFAULT_STALE_SIGNAL_SECS;
}
