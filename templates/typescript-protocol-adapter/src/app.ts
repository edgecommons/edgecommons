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
 * This module holds the adapter's **pure, unit-tested logic**: config parsing, the reconnect
 * backoff, the control mailbox, health/connectivity, the southbound publish path, and the
 * per-message control decisions ({@link handleControl}/{@link serveWhileDown}). The loop that drives
 * them — the connect/poll/reconnect supervisor per device — is the thin live-runtime seam in
 * `src/runtime.ts`, excluded from the coverage gate (it needs a live runtime; see `vitest.config.ts`).
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
  DataFacade,
  EventsFacade,
  InstanceConnectivity,
  Quality as LibQuality,
  Severity,
  logger,
} from "@edgecommons/edgecommons";

import {
  BrowseError,
  BrowsePage,
  ConnectionConfig,
  DeviceSession,
  Quality,
  Reading,
} from "./device";
import { DeviceMetrics } from "./metrics";

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

/** One poll: read, publish, record latencies + staleness. `polled` = signals published. */
export async function pollOnce(
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
export type PollExit =
  | { kind: "linkLost" }
  | { kind: "closed" }
  | { kind: "reconnect"; reply: (outcome: ReconnectOutcome) => void };

/** What servicing the control channel while the session is down concluded. */
export type DownOutcome =
  | { kind: "elapsed" }
  | { kind: "closed" }
  | { kind: "reconnect"; reply: (outcome: ReconnectOutcome) => void };

/**
 * Parse `component.instances[]` into device configs, skipping a malformed device with a warning.
 *
 * A malformed device is skipped rather than killing the component — but if EVERY device is malformed
 * there is nothing to run, and failing loudly beats idling silently.
 *
 * @throws Error when no device is valid
 */
export function buildDevices(config: Config): DeviceConfig[] {
  const devices: DeviceConfig[] = [];
  for (const id of config.instanceIds()) {
    try {
      devices.push(parseDevice(config.instance(id)));
    } catch (e) {
      logger.warn(`skipping malformed device '${id}': ${String(e)}`);
    }
  }
  if (devices.length === 0) {
    throw new Error("no valid devices in component.instances[]");
  }
  return devices;
}

/**
 * Service one control message during polling. Returns a {@link PollExit} only for reconnect /
 * link-lost; every other verb replies in place and returns `undefined` (stay in the poll loop).
 *
 * This is the per-message decision the poll loop delegates to — pure over its arguments (no runtime
 * state), so it is unit-tested directly against the sim session; the loop that feeds it lives in the
 * runtime seam (`src/runtime.ts`).
 */
export async function handleControl(
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
 *
 * Like {@link handleControl}, this is a per-message decision, pure over its arguments and
 * unit-tested directly; the connect/backoff supervisor that calls it lives in `src/runtime.ts`.
 */
export async function serveWhileDown(
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

/** Read `component.global.healthThresholds.staleSignalSecs` (default 30). */
export function readStaleSignalSecs(config: Config): number {
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
