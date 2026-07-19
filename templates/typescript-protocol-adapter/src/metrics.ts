/**
 * # Operational metrics — the canonical `southbound_health` + the operational-family pattern
 *
 * Every southbound adapter emits the shared {@link HEALTH} metric with **exactly** the
 * SOUTHBOUND.md §5 measure set. On top of that, this module ships the **operational-family
 * pattern** two protocols deep as worked examples — {@link CONNECTION} and {@link COMMAND} — and
 * shows you where to add your own.
 *
 * ## What `<<COMPONENTNAME>>` emits today
 *
 * | Metric | Dimensions | What it is |
 * |---|---|---|
 * | `southbound_health` | `instance` | the §5 canonical set (below) — every adapter emits this |
 * | `<<COMPONENTNAME>>Connection` | `instance` | the connect/reconnect lifecycle |
 * | `<<COMPONENTNAME>>Command` | `instance`, `verb`, `result` | the `sb/*` command surface |
 *
 * ## The Total/Interval counter convention
 *
 * Every **counter** is emitted as a measure PAIR: `<name>Total` (monotonic since start) and
 * `<name>Interval` (since the previous emit of that family; **reset on emit** — see {@link Pair}).
 * **Gauges** (`connectionState`) and interval **sums** (the `*Ms` latencies/durations) are single
 * measures. This is the same convention `modbus-adapter` and `ethernet-ip-adapter` use, so a fleet
 * dashboard reads every adapter the same way.
 *
 * ## Dimensions are LOW-CARDINALITY only
 *
 * `instance`, `verb` (the closed {@link COMMAND_VERBS} set), and `result` (`success`|`error`) — and
 * nothing else. **Never** dimension by signal name, address, endpoint, or error text: those are
 * unbounded and would shred a fleet dashboard. (`coreName`/`category`/`component` are injected by
 * `MetricBuilder.build`.)
 *
 * ## Add your protocol's families HERE
 *
 * `<<COMPONENTNAME>>Connection`/`Command` are generic — every adapter has them. Your protocol also
 * has an **inventory** (configured signals), a **poll/subscribe** path, and a **publish** path
 * worth measuring. Add `<<COMPONENTNAME>>Inventory` / `<<COMPONENTNAME>>Poll` /
 * `<<COMPONENTNAME>>Publish` families next to the two below — see
 * `modbus-adapter/modbus_adapter/metrics.py` and
 * `ethernet-ip-adapter/crates/ethernet-ip-adapter/src/metrics.rs` for the full worked set (poll
 * cycles, samples good/bad/uncertain/changed/suppressed, batch flushes, …). Register each new
 * family in {@link familyDefs} and pre-define it in {@link DeviceMetrics.defineAll}; the rest of the
 * pattern (record → drain → emit) is copy-shaped from {@link CmdCounters}.
 */
import { Config, MetricBuilder, MetricService, logger } from "@edgecommons/edgecommons";

import { Health } from "./app";

/** The metric every southbound adapter emits (SOUTHBOUND.md §5). */
export const HEALTH = "southbound_health";
/**
 * The worked operational family for the connect/reconnect lifecycle. Named from the component so a
 * fleet view can tell one adapter's connection health from another's.
 */
export const CONNECTION = "<<COMPONENTNAME>>Connection";
/** The worked operational family for the `sb/*` command surface, dimensioned `instance`×`verb`×`result`. */
export const COMMAND = "<<COMPONENTNAME>>Command";

/** A `result` dimension value: the operation succeeded. */
export const RESULT_SUCCESS = "success";
/** A `result` dimension value: the operation failed. */
export const RESULT_ERROR = "error";
const RESULTS: readonly string[] = [RESULT_SUCCESS, RESULT_ERROR];

/**
 * The **closed** `verb` dimension set for {@link COMMAND} — every `sb/*` verb the command surface
 * registers (`src/commands.ts`). Closed and low-cardinality on purpose (see the module header).
 */
export const COMMAND_VERBS: readonly string[] = [
  "sb/status",
  "sb/read",
  "sb/write",
  "sb/signals",
  "sb/browse",
  "sb/pause",
  "sb/resume",
  "reconnect",
  "repoll",
];

/**
 * The **exact** SOUTHBOUND.md §5 measure set of `southbound_health` — `connectionState`,
 * `publishLatencyMs`, `pollLatencyMs`, `readErrors`, `staleSignals`, plus the §5-optional
 * `reconnects`. This literal list is the parity anchor the metrics test asserts against; if you
 * change what `emitHealth` emits, this list and {@link familyDefs} must move with it.
 */
export const HEALTH_MEASURES: readonly string[] = [
  "connectionState",
  "publishLatencyMs",
  "pollLatencyMs",
  "readErrors",
  "staleSignals",
  "reconnects",
];

const UNIT_COUNT = "Count";
const UNIT_MS = "Milliseconds";

// =================================================================================================
// The definition schema — the single source the startup pre-definition and the parity test both read
// =================================================================================================

/** One measure's name, unit, and storage resolution. */
export interface MeasureDef {
  readonly name: string;
  readonly unit: string;
  readonly res: number;
}

/** One metric family's full definition: its name, dimension keys, and measures. */
export interface FamilyDef {
  readonly name: string;
  readonly dimensions: readonly string[];
  readonly measures: readonly MeasureDef[];
}

function m(name: string, unit: string, res: number): MeasureDef {
  return { name, unit, res };
}

/** A `<prefix>Total` + `<prefix>Interval` counter pair (both `Count`, resolution 60). */
function pairDefs(prefix: string): MeasureDef[] {
  return [m(`${prefix}Total`, UNIT_COUNT, 60), m(`${prefix}Interval`, UNIT_COUNT, 60)];
}

/**
 * The **complete** definition set — every family, measure, and dimension key this adapter emits.
 * The startup pre-definition ({@link DeviceMetrics.defineAll}) and the parity test both read it, so
 * a dropped or renamed measure fails the build.
 */
export function familyDefs(): FamilyDef[] {
  const out: FamilyDef[] = [];

  // southbound_health — the §5 canonical set (dims: instance). All single measures.
  out.push({
    name: HEALTH,
    dimensions: ["instance"],
    measures: [
      m("connectionState", UNIT_COUNT, 1),
      m("publishLatencyMs", UNIT_MS, 1),
      m("pollLatencyMs", UNIT_MS, 1),
      m("readErrors", UNIT_COUNT, 60),
      m("staleSignals", UNIT_COUNT, 60),
      m("reconnects", UNIT_COUNT, 60),
    ],
  });

  // <<COMPONENTNAME>>Connection — the connect/reconnect lifecycle (dims: instance).
  const conn: MeasureDef[] = [m("connectionState", UNIT_COUNT, 1)];
  conn.push(...pairDefs("connectAttempts"));
  conn.push(...pairDefs("connectFailures"));
  conn.push(...pairDefs("reconnectAttempts"));
  conn.push(...pairDefs("connectionDrops"));
  conn.push(m("connectedDurationMs", UNIT_MS, 60));
  out.push({ name: CONNECTION, dimensions: ["instance"], measures: conn });

  // <<COMPONENTNAME>>Command — the sb/* surface (dims: instance, verb, result).
  const cmd: MeasureDef[] = [];
  cmd.push(...pairDefs("commandRequests"));
  cmd.push(...pairDefs("commandErrors"));
  cmd.push(m("commandLatencyMs", UNIT_MS, 60));
  out.push({ name: COMMAND, dimensions: ["instance", "verb", "result"], measures: cmd });

  // ADD YOUR PROTOCOL'S FAMILIES HERE (Inventory / Poll / Publish — see the module header).

  return out;
}

function familyDef(name: string): FamilyDef {
  const def = familyDefs().find((f) => f.name === name);
  if (def === undefined) {
    throw new Error(`familyDefs must cover every family the emitter uses (missing '${name}')`);
  }
  return def;
}

// =================================================================================================
// Counter state
// =================================================================================================

/** A `<name>Total` (monotonic) + `<name>Interval` (reset on emit) counter pair. */
export class Pair {
  total = 0;
  interval = 0;

  add(v: number): void {
    this.total += v;
    this.interval += v;
  }

  /** Write both measures into `out` and **reset the interval** — the emit convention. */
  drainInto(out: Record<string, number>, prefix: string): void {
    out[`${prefix}Total`] = this.total;
    out[`${prefix}Interval`] = this.interval;
    this.interval = 0;
  }
}

class ConnCounters {
  everConnected = false;
  readonly connectAttempts = new Pair();
  readonly connectFailures = new Pair();
  readonly reconnectAttempts = new Pair();
  readonly connectionDrops = new Pair();
  connectedAccruedMs = 0;
  connectedSince?: number;

  private accrue(now: number): void {
    if (this.connectedSince !== undefined) {
      this.connectedAccruedMs += Math.max(0, now - this.connectedSince);
      this.connectedSince = now;
    }
  }

  drain(now: number, connectionState: number): Record<string, number> {
    this.accrue(now);
    const v: Record<string, number> = {};
    v.connectionState = connectionState;
    this.connectAttempts.drainInto(v, "connectAttempts");
    this.connectFailures.drainInto(v, "connectFailures");
    this.reconnectAttempts.drainInto(v, "reconnectAttempts");
    this.connectionDrops.drainInto(v, "connectionDrops");
    v.connectedDurationMs = this.connectedAccruedMs;
    this.connectedAccruedMs = 0;
    return v;
  }

  markConnected(now: number): void {
    this.connectedSince = now;
    if (this.everConnected) this.reconnectAttempts.add(1);
    this.everConnected = true;
  }

  markDropped(now: number): void {
    this.accrue(now);
    this.connectedSince = undefined;
    this.connectionDrops.add(1);
  }
}

class CmdCounters {
  readonly commandRequests = new Pair();
  readonly commandErrors = new Pair();
  commandLatencyMs = 0;

  drain(): Record<string, number> {
    const v: Record<string, number> = {};
    this.commandRequests.drainInto(v, "commandRequests");
    this.commandErrors.drainInto(v, "commandErrors");
    v.commandLatencyMs = this.commandLatencyMs;
    this.commandLatencyMs = 0;
    return v;
  }
}

/**
 * A per-device operational-metrics emitter. Owns the counter state for one device's
 * `southbound_health` plus the two worked families, and emits them on the metrics cadence and on
 * connect/disconnect transitions. One per configured instance.
 */
export class DeviceMetrics {
  private readonly conn = new ConnCounters();
  /** `${verb} ${result}` -> counters; pre-populated so the dimension set is fixed at startup. */
  private readonly command = new Map<string, CmdCounters>();
  /** Per-signal last-update epoch-ms — the staleness tracker driving `southbound_health.staleSignals`. */
  private readonly lastUpdate = new Map<string, number>();
  /** A signal with no update for longer than this counts in `staleSignals` (ms). */
  private readonly staleAfterMs: number;

  /**
   * Build the emitter for one device, pre-populating the full `(verb, result)` command matrix so
   * the dimension set is fixed and discoverable at startup.
   */
  constructor(
    private readonly svc: MetricService,
    private readonly config: Config,
    private readonly instance: string,
    private readonly health: Health,
    staleSignalSecs: number,
  ) {
    this.staleAfterMs = Math.max(1, staleSignalSecs) * 1000;
    for (const verb of COMMAND_VERBS) {
      for (const result of RESULTS) {
        this.command.set(cmdKey(verb, result), new CmdCounters());
      }
    }
  }

  // ---- recording (called from the device task; all synchronous) --------------------------------

  /** A connect attempt is about to be made. */
  onConnectAttempt(): void {
    this.conn.connectAttempts.add(1);
  }

  /**
   * The connect attempt succeeded. A re-establishment (after a previous drop) also bumps
   * `reconnectAttempts`.
   */
  onConnected(now: number): void {
    this.conn.markConnected(now);
  }

  /** The connect attempt failed (unreachable / refused / timeout). */
  onConnectFailure(): void {
    this.conn.connectFailures.add(1);
  }

  /** An established session was lost. */
  onConnectionDropped(now: number): void {
    this.conn.markDropped(now);
  }

  /** Note that a signal just updated — feeds the `staleSignals` tracker. */
  onSignalUpdate(signalId: string, now: number): void {
    this.lastUpdate.set(signalId, now);
  }

  /** Record one `sb/*` command outcome for its `(verb, result)` combo. */
  recordCommand(verb: string, ok: boolean, latencyMs: number): void {
    const result = ok ? RESULT_SUCCESS : RESULT_ERROR;
    let c = this.command.get(cmdKey(verb, result));
    if (c === undefined) {
      c = new CmdCounters();
      this.command.set(cmdKey(verb, result), c);
    }
    c.commandRequests.add(1);
    c.commandLatencyMs += latencyMs;
    if (!ok) c.commandErrors.add(1);
  }

  /**
   * The connection-counter snapshot for `sb/status` / the diagnostics panel: each counter as
   * `{interval, total}`. Cheap; no device I/O.
   */
  countersView(): Record<string, unknown> {
    const pair = (p: Pair): Record<string, number> => ({ interval: p.interval, total: p.total });
    return {
      connectAttempts: pair(this.conn.connectAttempts),
      connectFailures: pair(this.conn.connectFailures),
      reconnectAttempts: pair(this.conn.reconnectAttempts),
      connectionDrops: pair(this.conn.connectionDrops),
    };
  }

  private staleCount(now: number): number {
    let count = 0;
    for (const t of this.lastUpdate.values()) {
      if (now - t > this.staleAfterMs) count += 1;
    }
    return count;
  }

  // ---- definition + emission -------------------------------------------------------------------

  /**
   * Pre-define every family × dimension combination at startup, so the metric set is fixed and
   * discoverable. Each is also re-defined immediately before each emit (the name-keyed-store rule).
   */
  defineAll(): void {
    this.define(HEALTH, [["instance", this.instance]]);
    this.define(CONNECTION, [["instance", this.instance]]);
    for (const verb of COMMAND_VERBS) {
      for (const result of RESULTS) {
        this.define(COMMAND, [
          ["instance", this.instance],
          ["verb", verb],
          ["result", result],
        ]);
      }
    }
  }

  /** Build + register one family combo's metric definition. */
  private define(name: string, dimensions: ReadonlyArray<readonly [string, string]>): void {
    const def = familyDef(name);
    let b = MetricBuilder.create(name).withConfig(this.config);
    for (const measure of def.measures) {
      b = b.addMeasure(measure.name, measure.unit, measure.res);
    }
    for (const [k, v] of dimensions) {
      b = b.addDimension(k, v);
    }
    this.svc.defineMetric(b.build());
  }

  /** Re-define (with the combo's dimensions) then emit one family combo. */
  private async emitCombo(
    name: string,
    dimensions: ReadonlyArray<readonly [string, string]>,
    values: Record<string, number>,
    now: boolean,
  ): Promise<void> {
    this.define(name, dimensions);
    try {
      if (now) await this.svc.emitMetricNow(name, values);
      else await this.svc.emitMetric(name, values);
    } catch (e) {
      logger.warn(`metric emit failed metric=${name} instance=${this.instance}: ${String(e)}`);
    }
  }

  /**
   * The full periodic emit (every metrics interval): `southbound_health`, the connection family,
   * and every command `(verb, result)` combo.
   */
  async emitPeriodic(): Promise<void> {
    await this.emitHealth(false);
    await this.emitConnection(false);
    await this.emitCommand();
  }

  /**
   * The immediate transition emit (`emitMetricNow`): the mandatory `southbound_health` plus the
   * connection gauges whose state just changed — flushed on connect / disconnect.
   */
  async emitNow(): Promise<void> {
    await this.emitHealth(true);
    await this.emitConnection(true);
  }

  private async emitHealth(now: boolean): Promise<void> {
    const v: Record<string, number> = {
      connectionState: this.health.connectionState,
      publishLatencyMs: this.health.publishLatencyMs,
      pollLatencyMs: this.health.pollLatencyMs,
      readErrors: this.health.readErrors,
      staleSignals: this.staleCount(Date.now()),
      reconnects: this.health.reconnects,
    };
    // Interval counters reset on emit (parity with the §5 Count/60 measures).
    this.health.readErrors = 0;
    this.health.reconnects = 0;
    await this.emitCombo(HEALTH, [["instance", this.instance]], v, now);
  }

  private async emitConnection(now: boolean): Promise<void> {
    const state = this.health.connectionState;
    const values = this.conn.drain(Date.now(), state);
    await this.emitCombo(CONNECTION, [["instance", this.instance]], values, now);
  }

  private async emitCommand(): Promise<void> {
    const rows: Array<[string, string, Record<string, number>]> = [];
    for (const verb of COMMAND_VERBS) {
      for (const result of RESULTS) {
        const c = this.command.get(cmdKey(verb, result));
        if (c !== undefined) rows.push([verb, result, c.drain()]);
      }
    }
    for (const [verb, result, values] of rows) {
      await this.emitCombo(
        COMMAND,
        [
          ["instance", this.instance],
          ["verb", verb],
          ["result", result],
        ],
        values,
        false,
      );
    }
  }
}

function cmdKey(verb: string, result: string): string {
  return `${verb} ${result}`;
}
