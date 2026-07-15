/**
 * The library-owned `_bcast` republish listener — the UNS "late-join lever" (DESIGN-uns
 * §9.3 layer 2 / §9.4, DESIGN-uns-bridge §2.5): every component subscribes, on its PRIMARY
 * (local/IPC) connection, the two per-device broadcast command topics for its own device —
 *
 * ```text
 *   ecv1/{device}/_bcast/cmd/republish-state
 *   ecv1/{device}/_bcast/cmd/republish-cfg
 * ```
 *
 * and, on receipt, re-announces out of band: `republish-state` re-emits the heartbeat's `state`
 * keepalive (`{"status":"RUNNING","uptimeSecs":n}`) and `republish-cfg` re-runs the
 * effective-config (`cfg`) publisher. Both actions are supplied by the caller (`EdgeCommons`
 * wires `heartbeat.publishStateNow` / `effectiveConfigPublisher.publishNow`) and are expected to
 * go through the privileged reserved-publish seam themselves — this listener is just the
 * subscribe/jitter/coalesce plumbing. Mirrors the Java `com.mbreissi.edgecommons.uns.RepublishListener`.
 *
 * **Normative behavior (mirrored by the Java/Python/Rust listeners; constants pinned by
 * `uns-test-vectors/bcast.json`):**
 * - **Topics** — built through the {@link Uns} topic builder with the reserved `_bcast`
 *   pseudo-component identity: single-level hierarchy `[{level: "device", value: <own device>}]`,
 *   component {@link RepublishListener.BCAST_COMPONENT}, **component scope** (no instance token,
 *   D-U28), class `cmd`, channel =
 *   the verb. Always **rootless** (the identity is single-level, so `includeRoot` is a D-U25
 *   no-op — the broadcast topic shape is device-bus-wide, independent of any component's own
 *   hierarchy/root mode).
 * - **Trigger validation** — the envelope's `header.name` must equal the topic's verb
 *   ({@link RepublishListener.REPUBLISH_STATE} / {@link RepublishListener.REPUBLISH_CFG}); the
 *   header `version`, `body` and any `reply_to` are ignored (fire-and-forget notification, no
 *   reply). A missing/malformed message or a mismatched name is ignored (DEBUG log) — a
 *   malformed or foreign `_bcast` payload must never crash a component.
 * - **Jitter** — an accepted trigger fires after a uniformly random delay in
 *   `[0, JITTER_WINDOW_MS]` ms (the §9.3 "wait a random 0 to 2s" anti-stampede window: a whole
 *   fleet receives the broadcast at once). The randomness and clock are injected seams (no
 *   inline `Date.now()`/`Math.random()` calls in the decision logic) so the behavior
 *   unit-tests deterministically.
 * - **Coalescing / cooldown (per verb, independent)** — a trigger is accepted only when no
 *   re-announce for that verb is pending AND at least {@link RepublishListener.COOLDOWN_MS} ms
 *   have elapsed since the last **accepted** trigger for that verb (measured from acceptance,
 *   not from the jittered fire). Everything else coalesces into the pending/recent re-announce,
 *   so a looping or duplicated broadcast amplifies to at most one re-announce per verb per
 *   cooldown window.
 * - **No config surface** — always on; core plumbing, not a feature toggle. (The
 *   `republish-state` action still respects `heartbeat.enabled` — that check lives in
 *   `Heartbeat.publishStateNow`, not here.)
 *
 * Lifecycle: constructed and {@link start}ed by the `EdgeCommons` runtime after initialization
 * completes; {@link close} unsubscribes both topics (before messaging closes — the
 * unsubscribe-before-exit rule) and drops any pending re-announce. `start()` is best-effort and
 * idempotent: any failure (including a subscribe failure) logs a WARN and leaves the listener
 * disabled — the component must come up regardless.
 *
 * **TS-idiom divergence from Java**: the Java listener guards a `null` resolved component
 * identity (a mock/test bring-up state possible with Java's `ConfigManager`) and uses `synchronized`
 * blocks for thread-safety. Neither applies here: the TS `Config` model resolves
 * `componentIdentity` eagerly and fails fast at construction (see `config/model.ts`
 * `resolveComponentIdentity`), so a `Config` snapshot always carries a resolved identity — and
 * Node's single-threaded event loop needs no locking (the accept/coalesce decision in
 * {@link onBroadcast} runs to completion synchronously before yielding).
 */
import type { Config } from "./config/model";
import { logger } from "./logging";
import type { Message } from "./message";
import { MessageIdentity } from "./message";
import type { IMessagingService } from "./messaging/types";
import { Uns, UnsClass } from "./uns";

/**
 * The delayed-execution seam (the injected-clock discipline): production wraps a plain,
 * unref'd `setTimeout`; tests inject a recorder and run tasks synchronously.
 */
export type Delayer = (task: () => void, delayMillis: number) => void;

/** The injected clock seam (ms), the cooldown reference point. */
export type ClockMillis = () => number;

/** The injected jitter seam: returns a delay in `[0, windowMs]`. */
export type JitterFn = (windowMs: number) => number;

/** One broadcast verb's subscription + rate-limit state. */
interface Command {
  readonly verb: string;
  readonly action: () => void | Promise<void>;
  /** The resolved concrete topic; undefined until {@link RepublishListener.start} builds it. */
  topic?: string;
  /** A re-announce is scheduled but has not fired yet. */
  pending: boolean;
  /** Whether `lastAcceptedMs` holds a real acceptance time. */
  triggered: boolean;
  /** Clock millis of the last ACCEPTED trigger (the cooldown reference point). */
  lastAcceptedMs: number;
}

function errMsg(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/** Production delayer: a plain `setTimeout`, unref'd so it never keeps the process alive. */
function defaultDelayer(task: () => void, delayMillis: number): void {
  const timer = setTimeout(task, delayMillis);
  if (typeof timer.unref === "function") {
    timer.unref();
  }
}

/** Production jitter: a uniform integer in `[0, windowMs]`. */
function defaultJitter(windowMs: number): number {
  return Math.floor(Math.random() * (windowMs + 1));
}

export class RepublishListener {
  /** The reserved broadcast pseudo-component token (UNS-CANONICAL-DESIGN §4.3). */
  static readonly BCAST_COMPONENT = "_bcast";
  /** The re-announce-state broadcast verb (channel + envelope `header.name`). */
  static readonly REPUBLISH_STATE = "republish-state";
  /** The re-announce-effective-config broadcast verb (channel + envelope `header.name`). */
  static readonly REPUBLISH_CFG = "republish-cfg";
  /**
   * The anti-stampede jitter window in ms: an accepted broadcast re-announces after a uniformly
   * random delay in `[0, JITTER_WINDOW_MS]` (DESIGN-uns §9.3: "a random 0 to 2s"). Normative for
   * all four languages.
   */
  static readonly JITTER_WINDOW_MS = 2_000;
  /**
   * The per-verb coalescing cooldown in ms, measured from the last ACCEPTED trigger: at most one
   * re-announce per verb per this window, so a looping/duplicated broadcast never amplifies.
   * Normative for all four languages.
   */
  static readonly COOLDOWN_MS = 5_000;

  private readonly commands: Command[];
  private started = false;
  private closed = false;

  /**
   * @param configProvider  a getter for the current config snapshot (own-device identity
   *                        resolution; mirrors `Heartbeat`/`EffectiveConfigPublisher`)
   * @param messaging       the messaging service whose PRIMARY (local) connection carries the
   *                        subscriptions
   * @param stateRepublish  the `republish-state` action (the heartbeat's out-of-band state
   *                        keepalive re-emit)
   * @param cfgRepublish    the `republish-cfg` action (the effective-config publisher's
   *                        `publishNow`)
   * @param delayer         the injected delay seam (default: a real, unref'd `setTimeout`)
   * @param clockMillis     the injected clock seam (default: `Date.now`)
   * @param jitter          the injected jitter seam (default: `Math.random`-backed uniform)
   */
  constructor(
    private readonly configProvider: () => Config,
    private readonly messaging: IMessagingService,
    stateRepublish: () => void | Promise<void>,
    cfgRepublish: () => void | Promise<void>,
    private readonly delayer: Delayer = defaultDelayer,
    private readonly clockMillis: ClockMillis = () => Date.now(),
    private readonly jitter: JitterFn = defaultJitter,
  ) {
    this.commands = [
      {
        verb: RepublishListener.REPUBLISH_STATE,
        action: stateRepublish,
        pending: false,
        triggered: false,
        lastAcceptedMs: 0,
      },
      {
        verb: RepublishListener.REPUBLISH_CFG,
        action: cfgRepublish,
        pending: false,
        triggered: false,
        lastAcceptedMs: 0,
      },
    ];
  }

  /**
   * Builds the two own-device `_bcast` topics and subscribes them on the PRIMARY connection.
   * Best-effort and idempotent: on any failure (a subscription error, etc.) the listener logs a
   * WARN and disables itself; the component must come up regardless.
   */
  async start(): Promise<void> {
    if (this.started || this.closed) {
      return;
    }
    try {
      // The reserved _bcast pseudo-component pinned to this component's own device. The
      // identity is single-level, so the topic is rootless by construction (D-U25) - the
      // broadcast shape is shared by every component on the device bus, whatever their own
      // hierarchy/root mode.
      const identity = this.configProvider().componentIdentity;
      const bcastIdentity = new MessageIdentity(
        [{ level: "device", value: identity.device }],
        RepublishListener.BCAST_COMPONENT, // D-U28: _bcast is component scope (no instance)
      );
      const uns = new Uns(bcastIdentity, false);
      for (const command of this.commands) {
        const topic = uns.topic(UnsClass.Cmd, command.verb);
        await this.messaging.subscribe(topic, (_topic, message) => this.handle(command, message));
        command.topic = topic;
      }
      this.started = true;
      logger.info(`republish listener subscribed on '${this.commands[0].topic}' and '${this.commands[1].topic}'`);
    } catch (e) {
      logger.warn(`failed to start the _bcast republish listener (continuing without it): ${errMsg(e)}`);
    }
  }

  /**
   * One received broadcast: validate the envelope (`header.name` must equal the topic's verb),
   * then run the accept/coalesce decision. Never throws — a malformed or foreign `_bcast`
   * payload is ignored at DEBUG.
   */
  private handle(command: Command, message: Message): void {
    try {
      if (!message || !message.header || message.header.name !== command.verb) {
        logger.debug(`ignoring foreign/malformed _bcast payload on '${command.topic}'`);
        return;
      }
      this.onBroadcast(command);
    } catch (e) {
      logger.debug(`ignoring malformed _bcast payload on '${command.topic}': ${errMsg(e)}`);
    }
  }

  /**
   * The accept/coalesce decision (per verb): coalesce while a re-announce is pending or within
   * {@link RepublishListener.COOLDOWN_MS} ms of the last accepted trigger; otherwise accept and
   * schedule the re-announce after a jittered delay in `[0, JITTER_WINDOW_MS]` ms.
   */
  private onBroadcast(command: Command): void {
    if (this.closed) {
      return;
    }
    const now = this.clockMillis();
    if (command.pending) {
      logger.debug(`'${command.verb}' broadcast coalesced (a re-announce is already pending)`);
      return;
    }
    if (command.triggered && now - command.lastAcceptedMs < RepublishListener.COOLDOWN_MS) {
      logger.debug(`'${command.verb}' broadcast coalesced (within the ${RepublishListener.COOLDOWN_MS} ms cooldown)`);
      return;
    }
    command.pending = true;
    command.triggered = true;
    command.lastAcceptedMs = now;
    const delayMillis = this.jitter(RepublishListener.JITTER_WINDOW_MS);
    logger.debug(`'${command.verb}' broadcast accepted - re-announcing in ${delayMillis} ms`);
    this.delayer(() => this.fire(command), delayMillis);
  }

  /** The jittered re-announce: best-effort (a failing action must not wedge the verb). */
  private fire(command: Command): void {
    command.pending = false;
    if (this.closed) {
      return;
    }
    try {
      const result = command.action();
      if (result && typeof (result as Promise<void>).then === "function") {
        (result as Promise<void>).catch((e) => logger.warn(`'${command.verb}' re-announce failed: ${errMsg(e)}`));
      }
    } catch (e) {
      logger.warn(`'${command.verb}' re-announce failed: ${errMsg(e)}`);
    }
  }

  /**
   * Stops the listener: unsubscribes both `_bcast` topics (while messaging is still up — the
   * unsubscribe-before-exit rule) and drops any pending re-announce (the `closed` flag makes
   * {@link fire} a no-op even for an already-scheduled timer). Idempotent.
   */
  async close(): Promise<void> {
    if (this.closed) {
      return;
    }
    this.closed = true;
    if (this.started) {
      for (const command of this.commands) {
        if (command.topic) {
          try {
            await this.messaging.unsubscribe(command.topic);
          } catch (e) {
            logger.debug(`republish-listener unsubscribe of '${command.topic}' failed: ${errMsg(e)}`);
          }
        }
      }
    }
  }
}
