/**
 * <<COMPONENTNAME>> — application logic.
 *
 * This module holds the component's **pure, unit-tested logic**: the instance-connectivity provider
 * and the custom command verb's decision. The runtime that wires the `edgecommons` service handles
 * together and drives the demo loop lives next door in `src/runtime.ts` (the {@link module:runtime}
 * seam) — split out so the parts a test can exercise without a live runtime stay covered, and the
 * infinite loop that genuinely needs one stays out of the coverage denominator (see
 * `vitest.config.ts`).
 *
 * The `state` heartbeat keepalive AND the component command inbox are both **automatic**
 * (library-owned, no code here): the `state` keepalive publishes on
 * `ecv1/{device}/{component}/main/state` (on / 5 s / local by default), and the inbox
 * (`ecv1/{device}/{component}/main/cmd/#`, `gg.commands()`) already answers `ping` /
 * `reload-config` / `get-configuration` before the runtime's constructor even runs.
 *
 * What this scaffold adds is the rest of the monitoring + command surface the edge-console
 * reads (DESIGN-uns §7/§9 — G-S1/S2), so a freshly generated component has something to show up
 * on the console's Signals/Events/Metrics tabs and something custom to command, instead of an
 * empty dashboard:
 * - a periodic **metric** ({@link METRIC_NAME}: a monotonic `tickCount` counter plus an
 *   `uptimeSecs` gauge-like measure) via `gg.metrics()`;
 * - a periodic **data** signal ({@link DATA_SIGNAL_ID}: a sine-wave demo reading) via
 *   `gg.data()` — the `DataFacade` constructs the `SouthboundSignalUpdate` body
 *   (device/signal/samples) and defaults an omitted sample quality to `GOOD`, so the console's
 *   Signals tab has something to chart;
 * - a periodic **evt** (`ecv1/.../evt/info/sample-event`) via `gg.events()` — the
 *   `EventsFacade` derives the `evt/{severity}/{type}` channel from the body's own
 *   severity + type, so the topic and body can never disagree;
 * - a custom **command verb** ({@link SET_GREETING}), registered with `gg.commands().register(...)`
 *   alongside the automatic built-ins, whose decision is {@link applyGreeting}: it mutates a small
 *   piece of in-memory state which the periodic status publish then reflects on its very next tick —
 *   so invoking it from the console is visibly observable;
 * - an **instance-connectivity provider** ({@link instanceConnectivity}) — the one source both the
 *   `state` keepalive (push) and the built-in `status` verb (pull) read. This scaffold owns no
 *   connections, so it reports none; the function's docs show where a component that does adds them.
 *
 * Replace all four with your own business metrics/signals/events/verbs; none of this is required
 * by the library (a bare scaffold works fine without them), it exists so the demonstrated surface
 * is live end-to-end out of the box.
 */
import { CommandException, InstanceConnectivity } from "@edgecommons/edgecommons";

/** The demo loop-tick metric name (see the module docs). */
export const METRIC_NAME = "loopTicks";
/** The demo data() signal id (see the module docs). */
export const DATA_SIGNAL_ID = "demo-signal";
/** The custom command verb this scaffold registers (see the module docs). */
export const SET_GREETING = "set-greeting";
/** How often the demo loop ticks (publishes the status/metric/data/evt quartet), in ms. */
export const TICK_INTERVAL_MS = 10_000;
/** The greeting the demo loop starts with, mutated by {@link applyGreeting}. */
export const INITIAL_GREETING = "Hello from <<COMPONENTNAME>>";

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

/**
 * The result of a {@link SET_GREETING} command: the greeting before and after the change. A `type`
 * (not an `interface`) so it satisfies the library's `CommandResult` (`Record<string, unknown>`).
 */
export type GreetingChange = {
  previousGreeting: string;
  greeting: string;
};

/**
 * The decision behind the {@link SET_GREETING} command verb: validate the request body and compute
 * the greeting change. Pure — the runtime applies the returned {@link GreetingChange.greeting} to its
 * in-memory state and returns the object to the caller, so the effect lands on the next status tick.
 *
 * @throws CommandException `BAD_ARGS` when the body is not `{"greeting": "<text>"}`
 */
export function applyGreeting(body: unknown, current: string): GreetingChange {
  const next =
    typeof body === "object" && body !== null && typeof (body as Record<string, unknown>).greeting === "string"
      ? ((body as Record<string, unknown>).greeting as string)
      : undefined;
  if (next === undefined) {
    throw new CommandException("BAD_ARGS", 'expected a JSON body {"greeting": "<text>"}');
  }
  return { previousGreeting: current, greeting: next };
}
