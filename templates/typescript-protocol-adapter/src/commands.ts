/**
 * # The southbound command surface — the `sb/*` verbs + the three edge-console panels
 *
 * This module owns the whole `gg.commands()` registration for `<<COMPONENTNAME>>`: `sb/status`,
 * `sb/read`, `sb/write`, `sb/signals`, `sb/browse`, `sb/pause`, `sb/resume`, `reconnect`, `repoll`.
 * It is the generic southbound command family (SOUTHBOUND.md §2.2) every adapter serves — a real
 * adapter changes the *seam* behind it (`src/device.ts`), not this surface.
 *
 * ## Conventions every verb follows
 *
 * * **Instance routing (D-EIP-13):** `body.instance` is optional iff exactly one device is
 *   configured; with two or more, a missing id is `BAD_ARGS` and an unknown id is `NO_SUCH_INSTANCE`.
 * * **Standardized error codes:** `BAD_ARGS`, `NO_SUCH_INSTANCE`, `WRITE_NOT_ALLOWED`,
 *   `WRITE_FAILED`, `DEVICE_UNAVAILABLE`, `READ_FAILED`, `RECONNECT_FAILED`, `BROWSE_UNSUPPORTED`,
 *   `BROWSE_FAILED`.
 * * **The session is never touched here.** Every verb that reads/writes/reconnects/pauses is sent
 *   to the device's own loop as a {@link DeviceControl} and *confirmed* through the reply that rides
 *   it, because the session lives in that loop.
 * * **`sb/write` allow-lists BEFORE any device I/O.** A refused entry never becomes a
 *   {@link DeviceControl} write — an adapter that writes whatever it is asked to is a control-system
 *   vulnerability, not a feature.
 * * Every verb records into the `<<COMPONENTNAME>>Command` metric family (`instance`×`verb`×`result`).
 *
 * Three panels (`overview`, `signals`, `diagnostics`) are registered via `commands.registerPanel`
 * for the edge-console descriptor surface — each `scope: "instance"`, `order` 10/20/30.
 */
import { CommandException, CommandInbox, Message } from "@edgecommons/edgecommons";

import type {
  BrowseOutcome,
  DeviceConfig,
  DeviceControl,
  Health,
  Mailbox,
  ReadOutcome,
  ReconnectOutcome,
  RepollOutcome,
  WriteOutcome,
} from "./app";
import { Quality, Reading, SignalInfo } from "./device";
import type { DeviceMetrics } from "./metrics";

/**
 * The per-device handles the command surface routes on: the config (routing, allow-list,
 * inventory), the control channel (session-touching verbs), the shared health (status/paused), and
 * the metrics emitter (per-verb command counters).
 */
export interface DeviceHandle {
  readonly cfg: DeviceConfig;
  readonly control: Mailbox<DeviceControl>;
  readonly health: Health;
  readonly dm: DeviceMetrics;
  /** The signal inventory `sb/signals` returns — a config/backend view, no device round-trip. */
  readonly signals: readonly SignalInfo[];
}

/**
 * Register every `sb/*` verb + the three edge-console panels on the inbox.
 *
 * @throws Error / UnsValidationError when a verb/panel name clashes or a token is invalid.
 */
export function registerAll(commands: CommandInbox, handles: DeviceHandle[]): void {
  const commander = new Commander(handles);

  commands.register("sb/status", (req) => commander.status(req.body));
  commands.register("sb/read", (req) => commander.read(req.body));
  commands.register("sb/write", (req) => commander.write(req.body));
  commands.register("sb/signals", (req) => commander.signals(req.body));
  commands.register("sb/browse", (req) => commander.browse(req.body));
  // `sb/pause` additionally carries the requester identity path (for the emitted event's `by`).
  commands.register("sb/pause", (req: Message) => commander.pause(req.body, req.identity?.path));
  commands.register("sb/resume", (req) => commander.resume(req.body));
  commands.register("reconnect", (req) => commander.reconnect(req.body));
  commands.register("repoll", (req) => commander.repoll(req.body));

  for (const panel of panels()) commands.registerPanel(panel);
}

/**
 * The three edge-console panel descriptors. Core validates `id`/`title`/uniqueness; the widget
 * kinds and bound verbs are console-interpreted, so they ride verbatim. `order` 10/20/30,
 * `scope: "instance"`.
 */
export function panels(): Record<string, unknown>[] {
  return [
    {
      id: "overview",
      title: "Overview",
      order: 10,
      scope: "instance",
      widgets: [
        { kind: "summary", fields: ["connected", "state", "paused", "endpoint"] },
        { kind: "commandSummary", actions: ["reconnect", "sb/pause", "sb/resume"] },
      ],
      verbs: ["sb/status", "reconnect", "sb/pause", "sb/resume"],
    },
    {
      id: "signals",
      title: "Signals",
      order: 20,
      scope: "instance",
      widgets: [{ kind: "signalGrid" }],
      verbs: ["sb/signals", "sb/read", "sb/write", "repoll"],
    },
    {
      id: "diagnostics",
      title: "Diagnostics",
      order: 30,
      scope: "instance",
      widgets: [{ kind: "treeBrowser" }, { kind: "keyValueList" }],
      verbs: ["sb/browse", "sb/status"],
    },
  ];
}

type Reply = Record<string, unknown>;

/**
 * The command dispatcher: owns the per-device handles + the config order (for the single-instance
 * default). Exported so template tests can exercise each verb against a mock control loop directly.
 */
export class Commander {
  private readonly devices = new Map<string, DeviceHandle>();
  private readonly ids: string[];

  constructor(handles: DeviceHandle[]) {
    this.ids = handles.map((h) => h.cfg.id);
    for (const h of handles) this.devices.set(h.cfg.id, h);
  }

  /**
   * Route to the addressed device (D-EIP-13): `body.instance` optional iff exactly one device is
   * configured; with two or more a missing/unknown id is `BAD_ARGS` / `NO_SUCH_INSTANCE`.
   */
  private resolve(body: unknown): DeviceHandle {
    const o = asObject(body);
    const instance = o.instance;
    if (typeof instance === "string") {
      const h = this.devices.get(instance);
      if (!h) throw new CommandException("NO_SUCH_INSTANCE", `no configured device \`${instance}\``);
      return h;
    }
    if (this.ids.length === 1) return this.devices.get(this.ids[0]) as DeviceHandle;
    throw new CommandException("BAD_ARGS", "field `instance` is required when multiple devices are configured");
  }

  // --- sb/status ---------------------------------------------------------------------------------

  async status(body: unknown): Promise<Reply> {
    const h = this.resolve(body);
    const started = Date.now();
    const link = h.health.link();
    const connected = link === "ONLINE";
    const paused = h.health.isPaused();
    const state = paused && connected ? "PAUSED" : link;
    const out: Reply = {
      id: h.cfg.id,
      adapter: h.cfg.adapter,
      connected,
      state,
      paused,
      endpoint: h.cfg.connection.endpoint,
      metrics: h.dm.countersView(),
    };
    h.dm.recordCommand("sb/status", true, ms(started));
    return out;
  }

  // --- sb/signals (the configured inventory, no device I/O) --------------------------------------

  async signals(body: unknown): Promise<Reply> {
    const h = this.resolve(body);
    const started = Date.now();
    const signals = h.signals.map((s) => ({
      id: s.id,
      name: s.name ?? null,
      writable: h.cfg.writes.permits(s.id),
    }));
    h.dm.recordCommand("sb/signals", true, ms(started));
    return { id: h.cfg.id, signals };
  }

  // --- sb/read (on-demand read of named signals) ------------------------------------------------

  async read(body: unknown): Promise<Reply> {
    const h = this.resolve(body);
    const started = Date.now();
    const refs = asObject(body).signals;
    if (!Array.isArray(refs)) {
      throw new CommandException("BAD_ARGS", "expected a `signals` array");
    }

    // Resolve each ref to a stable id (keeping the request order for the reply).
    const plan = refs.map((r) => resolveRef(h, r));
    const ids = plan.filter((p): p is { id: string } => "id" in p).map((p) => p.id);

    const readings = new Map<string, Reading>();
    if (ids.length > 0) {
      const outcome = await send<ReadOutcome>(h, (reply) => ({ kind: "readNow", ids, reply }));
      if (!outcome.ok) {
        h.dm.recordCommand("sb/read", false, ms(started));
        throw new CommandException("READ_FAILED", outcome.error);
      }
      for (const r of outcome.readings) readings.set(r.signalId, r);
    }

    const reads = plan.map((entry) => {
      if ("id" in entry) {
        const r = readings.get(entry.id);
        return r !== undefined
          ? { signal: { id: entry.id }, value: r.value, quality: qualityStr(r.quality), qualityRaw: r.qualityRaw ?? null }
          : badRead(entry.id, "NO_DATA");
      }
      return badRead(entry.label, "UNRESOLVED_REF");
    });

    h.dm.recordCommand("sb/read", true, ms(started));
    return { id: h.cfg.id, reads };
  }

  // --- sb/write (§2.2 batch shape; allow-list BEFORE any device I/O; confirmed) ------------------

  async write(body: unknown): Promise<Reply> {
    const h = this.resolve(body);
    const started = Date.now();
    const entries = writeEntries(body);

    const results: Reply[] = [];
    let refused = 0;
    let attempted = 0;
    let succeeded = 0;

    for (const entry of entries) {
      const ref = resolveRef(h, entry);
      if (!("id" in ref)) {
        results.push({ signal: ref.label, ok: false, error: "unresolved ref" });
        continue;
      }
      const id = ref.id;
      // THE ALLOW-LIST — checked here, BEFORE the write ever reaches the device.
      if (!h.cfg.writes.permits(id)) {
        refused += 1;
        results.push({ signal: id, ok: false, error: "not in writes.allow" });
        continue;
      }
      if (!(typeof entry === "object" && entry !== null && "value" in entry)) {
        results.push({ signal: id, ok: false, error: "missing value" });
        continue;
      }
      const value = (entry as Record<string, unknown>).value;

      attempted += 1;
      const ack = await send<WriteOutcome>(h, (reply) => ({ kind: "write", signalId: id, value, ack: reply }));
      if (ack.ok) {
        succeeded += 1;
        results.push({ signal: id, value, ok: true });
      } else {
        results.push({ signal: id, value, ok: false, error: ack.error });
      }
    }

    // WRITE_NOT_ALLOWED only when EVERY entry was an allow-list refusal (nothing else attempted).
    if (entries.length > 0 && refused === entries.length) {
      h.dm.recordCommand("sb/write", false, ms(started));
      throw new CommandException("WRITE_NOT_ALLOWED", "no entry is in this instance's writes.allow list");
    }
    // WRITE_FAILED when every allowed write reached the device and every one failed.
    if (attempted > 0 && succeeded === 0) {
      h.dm.recordCommand("sb/write", false, ms(started));
      throw new CommandException("WRITE_FAILED", "every attempted write was rejected by the device");
    }

    h.dm.recordCommand("sb/write", true, ms(started));
    return { id: h.cfg.id, written: succeeded, results };
  }

  // --- sb/browse (paged address-space discovery) ------------------------------------------------

  async browse(body: unknown): Promise<Reply> {
    const h = this.resolve(body);
    const started = Date.now();
    const o = asObject(body);
    const cursor = typeof o.cursor === "string" ? o.cursor : undefined;
    const max = clamp(typeof o.max === "number" ? Math.floor(o.max) : 200, 1, 1000);

    const outcome = await send<BrowseOutcome>(h, (reply) => ({ kind: "browse", cursor, max, reply }));

    if (outcome.ok) {
      const entries = outcome.page.entries.map((e) => ({ id: e.id, name: e.name ?? null, type: e.typeName }));
      const out: Reply = { id: h.cfg.id, entries };
      if (outcome.page.nextCursor !== undefined) out.cursor = outcome.page.nextCursor;
      h.dm.recordCommand("sb/browse", true, ms(started));
      return out;
    }
    h.dm.recordCommand("sb/browse", false, ms(started));
    if (outcome.error.reason === "UNSUPPORTED") {
      throw new CommandException("BROWSE_UNSUPPORTED", "this adapter has no discovery service");
    }
    throw new CommandException("BROWSE_FAILED", outcome.error.message);
  }

  // --- sb/pause + sb/resume (idempotent {paused, changed}) --------------------------------------

  async pause(body: unknown, _by?: string): Promise<Reply> {
    const h = this.resolve(body);
    const started = Date.now();
    const changed = await send<boolean>(h, (reply) => ({ kind: "pause", reply }));
    h.dm.recordCommand("sb/pause", true, ms(started));
    return { id: h.cfg.id, paused: true, changed };
  }

  async resume(body: unknown): Promise<Reply> {
    const h = this.resolve(body);
    const started = Date.now();
    const changed = await send<boolean>(h, (reply) => ({ kind: "resume", reply }));
    h.dm.recordCommand("sb/resume", true, ms(started));
    return { id: h.cfg.id, paused: false, changed };
  }

  // --- reconnect ---------------------------------------------------------------------------------

  async reconnect(body: unknown): Promise<Reply> {
    const h = this.resolve(body);
    const started = Date.now();
    const outcome = await send<ReconnectOutcome>(h, (reply) => ({ kind: "reconnect", reply }));
    if (outcome.ok) {
      h.dm.recordCommand("reconnect", true, ms(started));
      return { id: h.cfg.id, connected: true };
    }
    h.dm.recordCommand("reconnect", false, ms(started));
    throw new CommandException("RECONNECT_FAILED", outcome.error);
  }

  // --- repoll (refused while paused) ------------------------------------------------------------

  async repoll(body: unknown): Promise<Reply> {
    const h = this.resolve(body);
    const started = Date.now();
    if (h.health.isPaused()) {
      h.dm.recordCommand("repoll", false, ms(started));
      throw new CommandException("BAD_ARGS", "instance is paused - resume first");
    }
    const outcome = await send<RepollOutcome>(h, (reply) => ({ kind: "repoll", reply }));
    if (outcome.ok) {
      h.dm.recordCommand("repoll", true, ms(started));
      return { id: h.cfg.id, polled: outcome.polled };
    }
    h.dm.recordCommand("repoll", false, ms(started));
    if (outcome.error.includes("paused")) {
      throw new CommandException("BAD_ARGS", outcome.error);
    }
    throw new CommandException("DEVICE_UNAVAILABLE", outcome.error);
  }
}

// =================================================================================================
// Helpers
// =================================================================================================

function ms(started: number): number {
  return Math.max(0, Date.now() - started);
}

function deviceUnavailable(): CommandException {
  return new CommandException("DEVICE_UNAVAILABLE", "device loop is unavailable");
}

/**
 * Send one control message and await the reply it carries. Building the message from an
 * inbox-provided `reply` callback keeps the "one reply per control message" contract; a closed
 * control channel (the device loop is gone) rejects as `DEVICE_UNAVAILABLE`.
 */
function send<T>(h: DeviceHandle, make: (reply: (value: T) => void) => DeviceControl): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const ctrl = make(resolve);
    if (!h.control.send(ctrl)) reject(deviceUnavailable());
  });
}

function qualityStr(q: Quality): string {
  // The `Quality` enum values are already the wire strings ("GOOD"/"BAD"/"UNCERTAIN").
  return q;
}

function badRead(id: string, raw: string): Reply {
  return { signal: { id }, value: null, quality: "BAD", qualityRaw: raw };
}

/**
 * Resolve a `sb/read`/`sb/write` signal-ref to its stable id: `{signalId}` / `{id}` directly, or
 * `{name}` looked up against the configured inventory. Returns a `label` for the BAD / unresolved
 * entry otherwise.
 */
function resolveRef(h: DeviceHandle, r: unknown): { id: string } | { label: string } {
  const o = asObject(r);
  if (typeof o.signalId === "string") return { id: o.signalId };
  if (typeof o.id === "string") return { id: o.id };
  if (typeof o.name === "string") {
    const s = h.signals.find((s) => s.name === o.name);
    return s ? { id: s.id } : { label: o.name };
  }
  return { label: "<invalid ref>" };
}

/**
 * Normalize an `sb/write` body to a list of `{ref…, value}` entries: a `writes` array, or a single
 * object carrying `value` (§2.2).
 *
 * @throws CommandException `BAD_ARGS` when neither form is present.
 */
function writeEntries(body: unknown): unknown[] {
  const o = asObject(body);
  if (Array.isArray(o.writes)) return o.writes;
  if ("value" in o) return [o];
  throw new CommandException("BAD_ARGS", "expected a `writes` array or a single write object with `value`");
}

function asObject(v: unknown): Record<string, unknown> {
  return typeof v === "object" && v !== null ? (v as Record<string, unknown>) : {};
}

function clamp(v: number, lo: number, hi: number): number {
  return Math.min(Math.max(v, lo), hi);
}
