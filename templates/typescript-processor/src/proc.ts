/**
 * # The pipeline: what a *processor* is
 *
 * A processor **subscribes**, **transforms**, and **forwards**. That is the whole archetype, and it
 * lives in three types:
 *
 * * {@link ProcMsg} — the unit that flows through the pipeline: a message plus the topic it arrived
 *   on.
 * * {@link Processor} — one stage. It takes a message and returns **zero or more** messages, so a
 *   stage can filter (return nothing), map (return one), or fan out (return several).
 * * {@link Pipeline} — an ordered chain of stages. The output of each stage is the input of the
 *   next.
 *
 * ## Why stages return an array and not `ProcMsg | undefined`
 *
 * A filter drops, a projection maps, an aggregator emits on a timer rather than on arrival. `0..N`
 * covers all three without a special case, and it is what lets {@link Processor.onTick} exist: a
 * *stateful* stage (a window, a debounce, a batch) accumulates in `process` and emits in `onTick`,
 * so time-driven output is not a different mechanism from data-driven output.
 *
 * ## One loop per route, so state needs no lock
 *
 * Each route owns its `Pipeline` in a single loop. Per-key state inside a stage is plain instance
 * state with no coordination anywhere, which is what makes a stateful stage cheap to write
 * correctly.
 */
import { Message } from "@edgecommons/edgecommons";

/**
 * A message in flight, and the topic it arrived on.
 *
 * The topic is carried because a stage may want to route on it, and because the dispatcher needs it
 * to decide where the result goes.
 */
export interface ProcMsg {
  /** The topic it arrived on. The demo stages ignore it; yours may want to route on it. */
  topic: string;
  msg: Message;
}

/** One stage of the pipeline. **This is the interface you implement.** */
export interface Processor {
  /** Handle one inbound message. Return what should continue downstream (zero, one, or many). */
  process(m: ProcMsg): ProcMsg[];

  /**
   * Called periodically, for stages that emit on time rather than on arrival (a window, a batch, a
   * debounce). Omit it — or return `[]` — for a stateless stage: a stage that ignores time.
   */
  onTick?(nowMs: number): ProcMsg[];
}

/** An ordered chain of stages. */
export class Pipeline {
  constructor(private readonly stages: readonly Processor[]) {}

  /**
   * Run a batch through every stage in order.
   *
   * When `nowMs` is given, each stage additionally gets an {@link Processor.onTick} after its data
   * pass, and whatever it emits joins the batch flowing downstream — so a window closing in stage 1
   * is still projected by stage 2 on the same pass, rather than waiting for the next message to
   * shake it loose.
   */
  run(input: readonly ProcMsg[], nowMs?: number): ProcMsg[] {
    let carried: ProcMsg[] = [...input];
    for (const stage of this.stages) {
      const next: ProcMsg[] = [];
      for (const m of carried) {
        next.push(...stage.process(m));
      }
      if (nowMs !== undefined && stage.onTick) {
        next.push(...stage.onTick(nowMs));
      }
      carried = next;
    }
    return carried;
  }
}

// --- Demo stages -----------------------------------------------------------------------------
//
// Two stages, enough to show both halves of the interface. Replace them with your own; nothing
// below is required by the library.

/**
 * Drops any message whose dotted body path does not equal an expected value.
 *
 * A filter is the simplest useful stage: it returns nothing, and the message stops there.
 */
export class FieldEquals implements Processor {
  constructor(
    private readonly path: string,
    private readonly value: unknown,
  ) {}

  process(m: ProcMsg): ProcMsg[] {
    const got = pluck(m.msg.body, this.path);
    return deepEquals(got, this.value) ? [m] : [];
  }
}

/**
 * Counts messages and emits a rollup on each tick.
 *
 * This is the stateful half of the interface: it accumulates in `process` (emitting nothing) and
 * produces its output in `onTick`. Windows, batches, and debounces are all this shape.
 */
export class CountPerTick implements Processor {
  private seen = 0;
  private last?: ProcMsg;

  process(m: ProcMsg): ProcMsg[] {
    this.seen += 1;
    this.last = m;
    return []; // nothing goes downstream on arrival — see onTick
  }

  onTick(_nowMs: number): ProcMsg[] {
    const last = this.last;
    const n = this.seen;
    this.last = undefined;
    this.seen = 0;
    if (!last || n === 0) return []; // an empty window is not an event
    // The body is rewritten in place; the envelope is NOT rebuilt here. The dispatcher restamps
    // identity when it publishes (a republisher's output is its own, not the producer's), and it
    // reads `header.name`/`header.version` off this message to do it.
    last.msg.body = { count: n, last: last.msg.body };
    return [last];
  }
}

/** Resolve a dotted path (`signal.id` style) inside a JSON body. */
export function pluck(value: unknown, path: string): unknown {
  let current = value;
  for (const segment of path.split(".")) {
    if (typeof current !== "object" || current === null) return undefined;
    current = (current as Record<string, unknown>)[segment];
  }
  return current;
}

/** Structural equality, so `fieldEquals` can match an object or array value, not just a scalar. */
function deepEquals(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (typeof a !== "object" || typeof b !== "object" || a === null || b === null) return false;
  return JSON.stringify(a) === JSON.stringify(b);
}

// --- stage config ----------------------------------------------------------------------------

/**
 * A stage, as named in config: a single-key object naming the stage and its arguments. Add a case
 * here as you add a stage.
 */
export type StageConfig =
  | { fieldEquals: { path: string; value: unknown } }
  | { countPerTick: Record<string, never> };

/**
 * Build one stage from its config entry.
 *
 * @throws Error when the entry names no known stage — a typo'd stage is a mistake, not a no-op.
 */
export function buildStage(cfg: unknown): Processor {
  if (typeof cfg !== "object" || cfg === null) throw new Error("a stage must be an object");
  const keys = Object.keys(cfg);
  if (keys.length !== 1) throw new Error("a stage must name exactly one stage kind");
  const o = cfg as Record<string, unknown>;

  switch (keys[0]) {
    case "fieldEquals": {
      const args = o.fieldEquals as { path?: unknown; value?: unknown };
      if (typeof args?.path !== "string") throw new Error("fieldEquals needs a `path`");
      if (!("value" in args)) throw new Error("fieldEquals needs a `value`");
      return new FieldEquals(args.path, args.value);
    }
    case "countPerTick":
      return new CountPerTick();
    default:
      throw new Error(`unknown stage '${keys[0]}'`);
  }
}
