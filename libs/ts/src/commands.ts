/**
 * The library-owned component **command inbox** — the minimal `commands()` facade
 * (DESIGN-uns §7.3 / §9.5, the edge-console slice S2): every component subscribes, on its
 * PRIMARY (local/IPC) connection, both its component-scope and instance-scope command-inbox
 * wildcards (D-U28)
 *
 * ```text
 *   ecv1/{device}/{component}/cmd/#
 *   ecv1/{device}/{component}/+/cmd/#
 * ```
 *
 * and dispatches incoming `cmd` envelopes to handlers by **verb** — the topic's channel
 * (everything after `cmd/`, `/`-namespaced verbs included), which the envelope's `header.name`
 * must equal. A request carrying `header.reply_to` gets a structured reply on that topic with
 * the request's `correlation_id` (the `uns-bridge` rewrites `reply_to` across brokers, so
 * console→component request/reply works transparently over the site bus); a `cmd` without
 * `reply_to` is fire-and-forget (the handler runs, no reply). Obtain the facade via
 * `EdgeCommons.commands()` and register custom verbs with {@link CommandInbox.register}. Mirrors
 * the Java `com.mbreissi.edgecommons.commands.CommandInbox`.
 *
 * **Normative behavior (mirrored by the Java/Python/Rust inboxes; pinned by
 * `uns-test-vectors/commands.json`):**
 * - **Reply body shape** — success `{"ok": true, "result": <verb-specific object>}`; error
 *   `{"ok": false, "error": {"code": <CODE>, "message": <text>}}`. The reply envelope's
 *   `header.name` is the verb, `header.version` is {@link CommandInbox.CMD_MESSAGE_VERSION}, and
 *   it carries the **responder's** `identity` (and `tags`, when configured — metadata, not
 *   normative).
 * - **Built-in verbs** (registered by the library, cannot be shadowed or unregistered):
 *   {@link CommandInbox.PING} → `{"status": "RUNNING", "uptimeSecs": n}` (liveness/echo, the
 *   state keepalive's RUNNING body shape); {@link CommandInbox.DESCRIBE} → return the
 *   component's command/panel discovery manifest; {@link CommandInbox.RELOAD_CONFIG} → re-fetch/
 *   re-apply the configuration from the active config source (`{"reloaded": true}` or
 *   {@link CommandInbox.ERR_RELOAD_FAILED}); {@link CommandInbox.GET_CONFIGURATION} → return
 *   the current **redacted effective config** as `{"config": <redacted config>}` — the same
 *   redacted snapshot the `cfg` push class publishes, as a reply (**Flow B**: the console pulls
 *   a component's own config; unrelated to the Flow-A
 *   `ecv1/{device}/config/cmd/get-configuration` rendezvous where a component fetches its
 *   config FROM a config server); {@link CommandInbox.STATUS} → `ping`'s per-instance superset,
 *   `{"status": "RUNNING", "uptimeSecs": n[, "instances": […]]}`, where `instances` is the very
 *   sample the `state` keepalive pushes (omitted when the component has no instances).
 * - **Unknown verb** — a well-formed request whose verb has no handler gets an
 *   {@link CommandInbox.ERR_UNKNOWN_VERB} error reply (fire-and-forget unknowns are ignored at
 *   DEBUG).
 * - **Malformed** — a missing header, a `header.name` that does not equal the topic's verb, or
 *   any parse anomaly is ignored at DEBUG, **never replied to and never a crash** (the G-S1
 *   precedent; replying would race foreign conventions that use a different header name on a
 *   `cmd` topic).
 * - **Delegated verbs** — {@link CommandInbox.SET_CONFIG_VERB} is owned by the
 *   `CONFIG_COMPONENT` config source's own subscription on the same inbox path; the inbox
 *   always ignores it (DEBUG) so the two subscribers never double-handle.
 * - **Handler errors** — a {@link CommandException} keeps its code; any other exception maps to
 *   {@link CommandInbox.ERR_HANDLER_ERROR}. Fire-and-forget failures are logged only.
 * - **No config surface** — always on; core plumbing, not a feature toggle.
 *
 * Lifecycle: constructed and {@link CommandInbox.start started} by the `EdgeCommonsBuilder` after
 * initialization completes; {@link CommandInbox.close} unsubscribes both inbox filters (before
 * messaging closes — the unsubscribe-before-exit rule). Under D-U28 the inbox subscribes both the
 * component-scope (`.../cmd/#`) and instance-scope (`.../+/cmd/#`) wildcards, so a command
 * addressed to the component or to any of its instances reaches the same dispatcher.
 *
 * **TS-idiom divergence from Java**: the Java inbox guards a `null` resolved component identity
 * (a mock/test bring-up state possible with Java's `ConfigManager`) and disables itself with a
 * WARN. Not applicable here: the TS `Config` model resolves `componentIdentity` eagerly and
 * fails fast at construction (`config/model.ts` `resolveComponentIdentity`), so a `Config`
 * snapshot always carries a resolved identity — mirrors the same divergence already documented
 * on `RepublishListener`. Handlers may be synchronous or return a `Promise` (Java handlers are
 * synchronous only); the built-in `reload-config` action is necessarily async (the TS config
 * sources are Promise-based), unlike Java's synchronous `ConfigManager.reloadFromProvider()`.
 */
import { createHash } from "crypto";
import { performance } from "perf_hooks";

import type { Config } from "./config/model";
import type { InstanceConnectivity } from "./instance_connectivity";
import { logger } from "./logging";
import type { Message } from "./message";
import { MessageBuilder } from "./message";
import type { IMessagingService } from "./messaging/types";
import { PublishConfirmationError } from "./messaging/types";
import { Uns, UnsClass, checkToken } from "./uns";

/** A verb handler's result: the verb-specific result object, or empty (`null`/`undefined`). */
export type CommandResult = Record<string, unknown> | null | undefined;

/**
 * A command-verb handler (DESIGN-uns §9.5): invoked by the {@link CommandInbox} for every
 * well-formed `cmd` envelope whose verb matches the registration.
 *
 * The return value is the verb-specific **result object**, wrapped by the inbox into the
 * success reply body `{"ok": true, "result": <returned object>}` and published to the
 * request's `header.reply_to` (with the request's `correlation_id`). Returning `null`/
 * `undefined` yields an empty result (`{"ok": true, "result": {}}` — a plain acknowledgement).
 * When the request carries no `reply_to` (fire-and-forget) the handler still runs but the
 * result is discarded.
 *
 * Failures: throw a {@link CommandException} for a coded error reply (`{"ok": false, "error":
 * {"code", "message"}}`); any other exception becomes the generic
 * {@link CommandInbox.ERR_HANDLER_ERROR} code. Handlers run on the messaging delivery path —
 * keep them fast, or hand off internally.
 *
 * @param request the full request envelope (body = the verb's arguments object; the
 *                requester's `identity`/`tags`, when present, are informational)
 * @returns the verb-specific result object (may be `null`/`undefined` for an empty result),
 *          synchronously or via a `Promise`
 */
export type CommandHandler = (request: Message) => CommandResult | Promise<CommandResult>;

/** Observable state of one inbox-owned deferred reply. */
export enum DeferredReplyState {
  Provisional = "PROVISIONAL",
  Open = "OPEN",
  Settling = "SETTLING",
  Settled = "SETTLED",
  Discarded = "DISCARDED",
  Expired = "EXPIRED",
  CancelledOnShutdown = "CANCELLED_ON_SHUTDOWN",
}

/** Observable lifecycle of the command inbox. */
export enum CommandInboxState {
  Starting = "STARTING",
  Active = "ACTIVE",
  Failed = "FAILED",
  Stopped = "STOPPED",
}

/** Result of asking an open token to settle. */
export enum SettlementResult {
  Accepted = "ACCEPTED",
  AlreadySettled = "ALREADY_SETTLED",
  Expired = "EXPIRED",
  CancelledOnShutdown = "CANCELLED_ON_SHUTDOWN",
  NotOpen = "NOT_OPEN",
}

/** Immediate standard command success. */
export interface ImmediateSuccess {
  readonly kind: "immediateSuccess";
  readonly result?: CommandResult;
}

/** Immediate standard coded command error. */
export interface ImmediateError {
  readonly kind: "immediateError";
  readonly code: string;
  readonly message: string;
}

/** Deferred settlement through an activated, inbox-issued opaque token. */
export interface Deferred {
  readonly kind: "deferred";
  readonly token: DeferredReply;
  /** Work the inbox starts only after it accepts this exact OPEN token. */
  readonly postAcceptContinuation?: PostAcceptContinuation;
}

/** Asynchronous work owned by the inbox after a deferred token is accepted. */
export type PostAcceptContinuation = () => void | Promise<void>;

/** Tagged explicit outcome of an {@link OutcomeCommandHandler}. */
export type CommandOutcome = ImmediateSuccess | ImmediateError | Deferred;

/** Factories for the tagged command outcomes. */
export const CommandOutcomes = {
  success(result?: CommandResult): ImmediateSuccess {
    return { kind: "immediateSuccess", result };
  },
  error(code: string, message: string): ImmediateError {
    if (!code) throw new Error("immediate error code must be non-empty");
    return { kind: "immediateError", code, message: message ?? "" };
  },
  deferred(token: DeferredReply): Deferred {
    if (!(token instanceof DeferredReply)) throw new Error("deferred outcome requires an inbox-issued token");
    return { kind: "deferred", token };
  },
  /**
   * Returns a deferred result whose continuation begins only after the inbox validates the exact
   * activated token for this delivery. The closure must settle its captured token through the
   * guarded API; it never receives a raw reply topic.
   */
  deferredWithContinuation(token: DeferredReply, postAcceptContinuation: PostAcceptContinuation): Deferred {
    if (!(token instanceof DeferredReply)) throw new Error("deferred outcome requires an inbox-issued token");
    if (typeof postAcceptContinuation !== "function") {
      throw new Error("post-accept continuation must be a function");
    }
    return { kind: "deferred", token, postAcceptContinuation };
  },
} as const;

/** Explicit-outcome command handler; may be synchronous or asynchronous. */
export type OutcomeCommandHandler = (request: Message) => CommandOutcome | Promise<CommandOutcome>;

interface DeferredEntry {
  readonly id: symbol;
  readonly verb: string;
  readonly correlationId: string;
  readonly replyTo: string;
  readonly requestUuid: string;
  readonly requestMetadata: Message;
  readonly expiresAtMs: number;
  state: DeferredReplyState;
  activated: boolean;
  cleaned: boolean;
  attempts: number;
  reply?: Message;
  expirationTimer?: NodeJS.Timeout;
  retryTimer?: NodeJS.Timeout;
}

/**
 * Opaque inbox-issued deferred-reply handle. It exposes lifecycle operations but neither the
 * retained reply topic nor a caller-controlled publish capability.
 */
export class DeferredReply {
  private constructor(
    private readonly owner: CommandInbox,
    private readonly entry: DeferredEntry,
  ) {}

  /** @internal */
  static _create(owner: CommandInbox, entry: DeferredEntry): DeferredReply {
    return new DeferredReply(owner, entry);
  }

  /** Activate only after application durable acceptance commits. */
  activate(): boolean {
    return this.owner._activateDeferred(this.entry);
  }

  /** Discard a still-provisional token after durable acceptance fails. */
  discard(): boolean {
    return this.owner._discardDeferred(this.entry);
  }

  /** Begin one standard success settlement. */
  settleSuccess(result?: CommandResult): SettlementResult {
    return this.owner._settleDeferred(this.entry, successBody(result));
  }

  /** Begin one standard coded error settlement. */
  settleError(code: string, message: string): SettlementResult {
    if (!code) throw new Error("deferred error code must be non-empty");
    return this.owner._settleDeferred(this.entry, errorBody(code, message));
  }

  /** Current state, including terminal state after registry cleanup. */
  state(): DeferredReplyState {
    return this.entry.state;
  }

  /** @internal */
  _validFor(owner: CommandInbox, request: Message, verb: string): boolean {
    if (this.owner !== owner || !this.entry.activated) return false;
    if (![DeferredReplyState.Open, DeferredReplyState.Settling, DeferredReplyState.Settled].includes(this.entry.state)) {
      return false;
    }
    return this.entry.verb === verb &&
      this.entry.replyTo === request.getReplyTo() &&
      this.entry.correlationId === request.getCorrelationId() &&
      this.entry.requestUuid === request.header.uuid;
  }

  /** @internal */
  _discardIfProvisional(owner: CommandInbox): void {
    if (this.owner === owner && this.entry.state === DeferredReplyState.Provisional) {
      this.discard();
    }
  }
}

/** Bounded deferred-registry counters for health/metrics. */
export interface DeferredReplySnapshot {
  readonly capacity: number;
  readonly active: number;
  readonly provisioned: number;
  readonly settled: number;
  readonly discarded: number;
  readonly expired: number;
  readonly openExpired: number;
  readonly cancelledOnShutdown: number;
  readonly capacityRejected: number;
}

/**
 * A coded command failure (DESIGN-uns §9.5): thrown by a {@link CommandHandler} to produce a
 * structured error reply `{"ok": false, "error": {"code": <code>, "message": <message>}}` with
 * a caller-chosen machine-readable code. Any **other** exception a handler throws is mapped to
 * the generic {@link CommandInbox.ERR_HANDLER_ERROR} code — this class exists so a handler
 * (built-in or custom) can distinguish its failure modes for the console (e.g.
 * {@link CommandInbox.ERR_RELOAD_FAILED}, {@link CommandInbox.ERR_NO_CONFIG}).
 */
export class CommandException extends Error {
  /** The machine-readable error code carried in the error reply's `error.code`. */
  readonly code: string;

  /**
   * @param code    the machine-readable error code (non-empty; SCREAMING_SNAKE_CASE by
   *                convention — see the pinned base codes on {@link CommandInbox})
   * @param message the human-readable message carried in the error reply's `error.message`
   */
  constructor(code: string, message: string) {
    super(message);
    this.name = "CommandException";
    if (!code) {
      throw new Error("code must not be empty");
    }
    this.code = code;
  }
}

function errMsg(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/** The error reply body `{"ok": false, "error": {"code", "message"}}`. */
function errorBody(code: string, message: string): Record<string, unknown> {
  return { ok: false, error: { code, message: message ?? "" } };
}

/** The standard success wrapper, snapshotting caller-owned result data before async retry. */
function successBody(result: CommandResult): Record<string, unknown> {
  return { ok: true, result: result == null ? {} : cloneCommandValue(result) };
}

function cloneCommandValue(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(cloneCommandValue);
  if (value !== null && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value as Record<string, unknown>).map(([key, item]) => [key, cloneCommandValue(item)]),
    );
  }
  return value;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function stableJson(value: unknown): string {
  if (value === null || typeof value !== "object") {
    return JSON.stringify(value) ?? "null";
  }
  if (Array.isArray(value)) {
    return `[${value.map((item) => stableJson(item === undefined ? null : item)).join(",")}]`;
  }
  const record = value as Record<string, unknown>;
  return `{${Object.keys(record)
    .filter((key) => record[key] !== undefined)
    .sort()
    .map((key) => `${JSON.stringify(key)}:${stableJson(record[key])}`)
    .join(",")}}`;
}

function digestDescribePayload(commands: Record<string, unknown>[], panels: Record<string, unknown>): string {
  const input = stableJson({ commands, panels });
  return `sha256:${createHash("sha256").update(input).digest("hex")}`;
}

function settlementResultFor(state: DeferredReplyState): SettlementResult {
  switch (state) {
    case DeferredReplyState.Settling:
    case DeferredReplyState.Settled:
      return SettlementResult.AlreadySettled;
    case DeferredReplyState.Expired:
      return SettlementResult.Expired;
    case DeferredReplyState.CancelledOnShutdown:
      return SettlementResult.CancelledOnShutdown;
    case DeferredReplyState.Provisional:
    case DeferredReplyState.Open:
    case DeferredReplyState.Discarded:
      return SettlementResult.NotOpen;
  }
}

function isDeferredTerminal(state: DeferredReplyState): boolean {
  return state === DeferredReplyState.Settled ||
    state === DeferredReplyState.Discarded ||
    state === DeferredReplyState.Expired ||
    state === DeferredReplyState.CancelledOnShutdown;
}

/**
 * Bounds one transport subscription acknowledgement without treating an elapsed timer as a
 * successful subscription. The caller owns cleanup because the public transport API has no
 * cancellation primitive for a subscribe already in progress.
 */
function awaitSubscriptionAcknowledgement(subscription: Promise<void>, timeoutMs: number): Promise<void> {
  return new Promise<void>((resolve, reject) => {
    let settled = false;
    const timer = setTimeout(() => {
      if (settled) return;
      settled = true;
      reject(new Error("command inbox subscription acknowledgement timed out"));
    }, timeoutMs);
    subscription.then(
      () => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        resolve();
      },
      (error: unknown) => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        reject(error);
      },
    );
  });
}

export class CommandInbox {
  /** The liveness/echo built-in verb. */
  static readonly PING = "ping";
  /** The component command/panel discovery built-in verb. */
  static readonly DESCRIBE = "describe";
  /** The re-fetch/re-apply-configuration built-in verb. */
  static readonly RELOAD_CONFIG = "reload-config";
  /** The return-my-redacted-effective-config built-in verb (Flow B). */
  static readonly GET_CONFIGURATION = "get-configuration";
  /**
   * The universal component status built-in verb:
   * `{"status": "RUNNING", "uptimeSecs": n[, "instances": […]]}`.
   *
   * {@link CommandInbox.PING} answers only for the component as a whole. `status` is its
   * per-instance superset: it returns the same sample the `state` keepalive pushes in `instances`,
   * sourced from the one component-supplied `InstanceConnectivityProvider` (through
   * `Heartbeat.sampleInstanceConnectivity`). Push and pull can therefore never disagree — a console
   * can subscribe, or ask, and get the same answer.
   *
   * Every component implements it by registering that provider; a component with no instances (a
   * plain service) simply omits the section and answers exactly as `ping` does. It is deliberately
   * **not** named `sb/status`: a processor or a sink has no southbound, and this verb is universal.
   */
  static readonly STATUS = "status";
  /** The command request/reply envelope version. */
  static readonly CMD_MESSAGE_VERSION = "1.0";
  /** Error code: the request's verb has no registered handler on this component. */
  static readonly ERR_UNKNOWN_VERB = "UNKNOWN_VERB";
  /** Error code: the handler threw an uncoded exception. */
  static readonly ERR_HANDLER_ERROR = "HANDLER_ERROR";
  /** Error code: {@link CommandInbox.RELOAD_CONFIG} could not re-fetch or the document was rejected. */
  static readonly ERR_RELOAD_FAILED = "RELOAD_FAILED";
  /** Error code: {@link CommandInbox.GET_CONFIGURATION} found no effective configuration to return. */
  static readonly ERR_NO_CONFIG = "NO_CONFIG";
  /** Error code: a deferred command was sent without a guarded reply target. */
  static readonly ERR_REPLY_REQUIRED = "REPLY_REQUIRED";
  /** Error code: bounded deferred-reply capacity is exhausted. */
  static readonly ERR_DEFERRED_REPLY_CAPACITY = "RESOURCE_LIMIT";
  /** Maximum inbox-owned post-accept continuations that may be in flight. */
  static readonly MAX_POST_ACCEPT_CONTINUATIONS = 256;
  /** Error attempted for open deferred replies during shutdown. */
  static readonly ERR_COMPONENT_STOPPING = "COMPONENT_STOPPING";
  /** Hard bound on active provisional/open/settling deferred replies. */
  static readonly MAX_DEFERRED_REPLIES = 1024;
  /** Bounded wait for the selected transport to acknowledge the inbox subscription. */
  static readonly DEFAULT_START_TIMEOUT_MS = 10_000;
  /** Hard bound for deliveries received after a subscribe request but before ACTIVE. */
  static readonly MAX_PENDING_STARTUP_DELIVERIES = 256;
  /** Camera-design upper bound (31 minutes) for one deferred reply lifetime. */
  static readonly MAX_DEFERRED_REPLY_LIFETIME_MS = 1_860_000;
  private static readonly DEFERRED_ATTEMPT_TIMEOUT_MS = 5_000;
  private static readonly DEFERRED_RETRY_INITIAL_MS = 100;
  private static readonly DEFERRED_RETRY_MAX_MS = 1_000;
  private static readonly DEFERRED_SHUTDOWN_TIMEOUT_MS = 1_000;
  /**
   * The `set-config` push verb — delegated: the `CONFIG_COMPONENT` config source maintains its
   * own subscription for it on the same inbox path, so the inbox must never dispatch or
   * error-reply it.
   */
  static readonly SET_CONFIG_VERB = "set-config";
  /** The built-in verbs (registered at construction; shadowing/unregistering is rejected). */
  static readonly BUILT_IN_VERBS: ReadonlySet<string> = new Set([
    CommandInbox.PING,
    CommandInbox.DESCRIBE,
    CommandInbox.RELOAD_CONFIG,
    CommandInbox.GET_CONFIGURATION,
    CommandInbox.STATUS,
  ]);
  /** Verbs owned by other library subscriptions on the same inbox path — always ignored. */
  static readonly DELEGATED_VERBS: ReadonlySet<string> = new Set([CommandInbox.SET_CONFIG_VERB]);

  /** verb -> handler; built-ins seeded at construction, custom verbs via {@link register}. */
  private readonly handlers = new Map<string, CommandHandler>();
  /** verb -> explicit-outcome handler; custom verbs via {@link registerOutcome}. */
  private readonly outcomeHandlers = new Map<string, OutcomeCommandHandler>();
  /** panel id -> descriptor; panel descriptors are carried verbatim for `describe`. */
  private readonly panelViews = new Map<string, Record<string, unknown>>();
  /** The instance-scoped inbox filter (`…/+/cmd/#`); undefined until {@link start} builds it. */
  private inboxFilter?: string;
  /** The component-scoped inbox filter (`…/cmd/#`, D-U28); undefined until {@link start} builds it. */
  private componentInboxFilter?: string;
  private started = false;
  private closed = false;
  /** The single in-flight startup attempt, shared by concurrent callers. */
  private startPromise?: Promise<void>;
  /** Deliveries retained until the selected transport acknowledges the subscription. */
  private activationPending: Array<{ topic: string; message: Message }> = [];
  private readonly deferredEntries = new Map<symbol, DeferredEntry>();
  private deferredProvisioned = 0;
  private deferredSettled = 0;
  private deferredDiscarded = 0;
  private deferredExpired = 0;
  private deferredOpenExpired = 0;
  private deferredCancelledOnShutdown = 0;
  private deferredCapacityRejected = 0;
  private postAcceptOutstanding = 0;
  private inboxState = CommandInboxState.Stopped;
  private inboxStartupError?: string;

  /**
   * Creates the inbox and registers the built-in verbs. The verb **actions** are injected
   * seams so the built-ins unit-test deterministically; `EdgeCommonsBuilder` wires the real ones.
   *
   * @param configProvider  a getter for the current config snapshot (own identity resolution;
   *                        reply envelopes are config-stamped with the responder's
   *                        identity/tags; mirrors `Heartbeat`/`RepublishListener`)
   * @param messaging       the messaging service whose PRIMARY connection carries the inbox
   * @param uptimeSecs      the {@link CommandInbox.PING} uptime source (production: the
   *                        heartbeat's monotonic uptime, `Heartbeat.getUptimeSecs`)
   * @param configReload    the {@link CommandInbox.RELOAD_CONFIG} action — re-fetch + re-apply
   *                        from the active config source, resolving `true` on success
   *                        (production: re-fetch the active `ConfigSource` and apply through
   *                        the same validate/apply/notify path a push hot-reload uses)
   * @param redactedConfig  the {@link CommandInbox.GET_CONFIGURATION} source — the current
   *                        redacted effective config, or `undefined` when unavailable
   *                        (production: `EffectiveConfigPublisher.redactedEffectiveConfig`)
   * @param instanceConnectivity the {@link CommandInbox.STATUS} source — the live per-instance
   *                        connectivity sample (production: `Heartbeat.sampleInstanceConnectivity`,
   *                        i.e. the very same provider the `state` keepalive pushes, so the pulled
   *                        answer and the pushed one cannot diverge). Defaults to "no instances",
   *                        which makes `status` answer exactly as `ping` does.
   */
  constructor(
    private readonly configProvider: () => Config,
    private readonly messaging: IMessagingService,
    uptimeSecs: () => number,
    configReload: () => boolean | Promise<boolean>,
    redactedConfig: () => Record<string, unknown> | undefined,
    instanceConnectivity: () => InstanceConnectivity[] | undefined | null = () => [],
    private readonly stateListener?: (state: CommandInboxState) => void,
  ) {
    // ping -> the state keepalive's RUNNING body shape: proves the component is not just alive
    // (the keepalive does that) but RESPONSIVE to addressed commands.
    this.handlers.set(CommandInbox.PING, () => ({
      status: "RUNNING",
      uptimeSecs: uptimeSecs(),
    }));
    // status -> ping's per-instance superset. Same body, plus the `instances` the state keepalive
    // pushes, from the same provider. A component with no instances omits the section, so a plain
    // service answers exactly as ping does.
    this.handlers.set(CommandInbox.STATUS, () => {
      const result: Record<string, unknown> = {
        status: "RUNNING",
        uptimeSecs: uptimeSecs(),
      };
      const conns = instanceConnectivity();
      if (conns && conns.length > 0) {
        const instances = conns.filter((c) => c != null).map((c) => c.toJson());
        if (instances.length > 0) {
          result.instances = instances;
        }
      }
      return result;
    });
    // describe -> command/panel discovery manifest for descriptor-driven console panels.
    this.handlers.set(CommandInbox.DESCRIBE, () => this.describe());
    // get-configuration (Flow B) -> the cfg class's body shape, as a reply.
    this.handlers.set(CommandInbox.GET_CONFIGURATION, () => {
      const config = redactedConfig();
      if (config === undefined) {
        throw new CommandException(CommandInbox.ERR_NO_CONFIG, "no effective configuration is available");
      }
      return { config };
    });
    // reload-config -> re-fetch from the active config source and re-apply (listeners fire, so
    // a successful reload also re-announces the cfg push as a side effect).
    this.handlers.set(CommandInbox.RELOAD_CONFIG, async () => {
      const ok = await configReload();
      if (!ok) {
        throw new CommandException(
          CommandInbox.ERR_RELOAD_FAILED,
          "the configuration could not be re-fetched from the active config source or the" +
            " document was rejected - see the component log",
        );
      }
      return { reloaded: true };
    });
  }

  /** Current startup state; ACTIVE means the exact transport filter was acknowledged. */
  state(): CommandInboxState {
    return this.inboxState;
  }

  /** Sanitized stable error available only while the inbox is FAILED. */
  startupError(): string | undefined {
    return this.inboxState === CommandInboxState.Failed ? this.inboxStartupError : undefined;
  }

  /** @internal Used when a builder command configurator rejects before subscription. */
  failStartup(): void {
    if (this.closed || this.inboxState === CommandInboxState.Active) return;
    this.transition(CommandInboxState.Failed, "COMMAND_INBOX_CONFIGURATION_FAILED");
  }

  private transition(state: CommandInboxState, error?: string): void {
    this.inboxState = state;
    this.inboxStartupError = state === CommandInboxState.Failed ? error : undefined;
    try {
      this.stateListener?.(state);
    } catch (e) {
      // Health observation cannot affect the command-plane lifecycle.
      logger.debug(`command inbox state listener failed: ${errMsg(e)}`);
    }
  }

  /**
   * Registers a custom verb handler — the minimal `commands()` registration seam. The verb is
   * one or more `/`-separated channel tokens (`"restart-pipeline"`, `"sb/status"`), each
   * validated against the §2.2 token rule. Registration is allowed before or after
   * {@link start} (the inbox is a single wildcard subscription — no per-verb subscribe).
   *
   * **Precedence:** no shadowing, ever — registering a {@link CommandInbox.BUILT_IN_VERBS
   * built-in}, a {@link CommandInbox.DELEGATED_VERBS delegated} or an already-registered verb
   * throws. Replace a custom handler by {@link unregister} first.
   *
   * @param verb    the verb (the `cmd` channel, `/`-namespaces allowed)
   * @param handler the handler to dispatch it to
   * @throws Error when the verb is built-in/delegated/already registered
   * @throws UnsValidationError when a verb token violates the §2.2 token rule
   */
  register(verb: string, handler: CommandHandler): void {
    this.validateCustomVerbRegistration(verb);
    this.handlers.set(verb, handler);
    logger.debug(`command verb '${verb}' registered`);
  }

  /** Register an explicit-outcome handler without changing the legacy {@link register} API. */
  registerOutcome(verb: string, handler: OutcomeCommandHandler): void {
    this.validateCustomVerbRegistration(verb);
    this.outcomeHandlers.set(verb, handler);
    logger.debug(`outcome command verb '${verb}' registered`);
  }

  private validateCustomVerbRegistration(verb: string): void {
    for (const token of verb.split("/")) {
      checkToken(token, "verb token");
    }
    if (CommandInbox.BUILT_IN_VERBS.has(verb)) {
      throw new Error(`verb '${verb}' is a built-in verb and cannot be shadowed`);
    }
    if (CommandInbox.DELEGATED_VERBS.has(verb)) {
      throw new Error(`verb '${verb}' is owned by another library subsystem and cannot be registered`);
    }
    if (this.handlers.has(verb) || this.outcomeHandlers.has(verb)) {
      throw new Error(`verb '${verb}' is already registered - unregister it first to replace the handler`);
    }
  }

  /**
   * Removes a previously registered custom verb handler. Unknown verbs are a no-op; built-in
   * verbs cannot be unregistered.
   *
   * @param verb the custom verb to remove
   * @throws Error when the verb is a built-in
   */
  unregister(verb: string): void {
    if (CommandInbox.BUILT_IN_VERBS.has(verb)) {
      throw new Error(`verb '${verb}' is a built-in verb and cannot be unregistered`);
    }
    if (this.handlers.delete(verb) || this.outcomeHandlers.delete(verb)) {
      logger.debug(`command verb '${verb}' unregistered`);
    }
  }

  /** The currently registered verbs (built-ins + custom) — a snapshot copy. */
  verbs(): Set<string> {
    return new Set([...this.handlers.keys(), ...this.outcomeHandlers.keys()]);
  }

  /**
   * Provision an opaque PROVISIONAL reply token before durable acceptance. Activate it only after
   * the durable insert commits, or discard it on failure.
   */
  defer(request: Message, lifetimeMs: number): DeferredReply {
    if (this.closed) {
      throw new CommandException(CommandInbox.ERR_COMPONENT_STOPPING, "the command inbox is stopping");
    }
    const replyTo = request?.getReplyTo();
    if (!replyTo) {
      throw new CommandException(CommandInbox.ERR_REPLY_REQUIRED, "deferred commands require a non-empty reply_to");
    }
    if (!Number.isInteger(lifetimeMs) || lifetimeMs < 1 || lifetimeMs > CommandInbox.MAX_DEFERRED_REPLY_LIFETIME_MS) {
      throw new Error(
        `deferred reply lifetimeMs must be between 1 and ${CommandInbox.MAX_DEFERRED_REPLY_LIFETIME_MS}`,
      );
    }
    const verb = request.header?.name;
    const correlationId = request.header?.correlation_id;
    const requestUuid = request.header?.uuid;
    if (!verb) throw new Error("deferred request requires a non-empty verb");
    if (!correlationId) throw new Error("deferred request requires a non-empty correlation id");
    if (!requestUuid) throw new Error("deferred request requires a non-empty message uuid");

    const validateTarget = this.messaging.validateReplyTarget;
    if (typeof validateTarget !== "function") {
      throw new PublishConfirmationError(
        "unsupported",
        "messaging service cannot guard deferred reply targets",
      );
    }
    validateTarget.call(this.messaging, request);
    if (this.deferredEntries.size >= CommandInbox.MAX_DEFERRED_REPLIES) {
      this.deferredCapacityRejected++;
      throw new CommandException(
        CommandInbox.ERR_DEFERRED_REPLY_CAPACITY,
        "deferred reply registry capacity is exhausted",
      );
    }

    const id = Symbol("deferred-reply");
    const entry: DeferredEntry = {
      id,
      verb,
      correlationId,
      replyTo,
      requestUuid,
      requestMetadata: MessageBuilder.create(verb, CommandInbox.CMD_MESSAGE_VERSION)
        .withCorrelationId(correlationId)
        .withReplyTo(replyTo)
        .build(),
      expiresAtMs: performance.now() + lifetimeMs,
      state: DeferredReplyState.Provisional,
      activated: false,
      cleaned: false,
      attempts: 0,
    };
    entry.expirationTimer = setTimeout(() => this.expireDeferred(entry), lifetimeMs);
    if (typeof entry.expirationTimer.unref === "function") entry.expirationTimer.unref();
    this.deferredEntries.set(id, entry);
    this.deferredProvisioned++;
    return DeferredReply._create(this, entry);
  }

  /** Current bounded deferred-registry counters. */
  deferredReplySnapshot(): DeferredReplySnapshot {
    return {
      capacity: CommandInbox.MAX_DEFERRED_REPLIES,
      active: this.deferredEntries.size,
      provisioned: this.deferredProvisioned,
      settled: this.deferredSettled,
      discarded: this.deferredDiscarded,
      expired: this.deferredExpired,
      openExpired: this.deferredOpenExpired,
      cancelledOnShutdown: this.deferredCancelledOnShutdown,
      capacityRejected: this.deferredCapacityRejected,
    };
  }

  /**
   * Registers a descriptor-driven component-detail panel view. The core library validates only
   * the portable discovery contract (`id`, `title`, duplicate `id`) and carries the remaining
   * descriptor fields verbatim for the console-owned renderer.
   *
   * @param panel a JSON-object panel descriptor with non-empty string `id` and `title`
   * @throws Error when the panel is not an object, `id`/`title` is invalid, or the id is duplicate
   */
  registerPanel(panel: Record<string, unknown>): void {
    if (!isRecord(panel)) {
      throw new Error("panel must be a JSON object");
    }
    const id = panel.id;
    if (typeof id !== "string" || id.length === 0) {
      throw new Error("panel id must be a non-empty string");
    }
    const title = panel.title;
    if (typeof title !== "string" || title.length === 0) {
      throw new Error("panel title must be a non-empty string");
    }
    if (this.panelViews.has(id)) {
      throw new Error(`panel id '${id}' is already registered`);
    }
    this.panelViews.set(id, { ...panel });
  }

  /** The currently registered panel descriptors — a snapshot copy. */
  panels(): Record<string, unknown>[] {
    return [...this.panelViews.values()].map((panel) => ({ ...panel }));
  }

  private describe(): Record<string, unknown> {
    const config = this.configProvider();
    const identity = config.componentIdentity;
    const commands = [...this.verbs()].sort().map((verb) => ({
      verb,
      builtIn: CommandInbox.BUILT_IN_VERBS.has(verb),
    }));
    const views = this.panels();
    const defaultView = (views.find((view) => view.default === true) ?? views[0])?.id;
    const panels = {
      schemaVersion: "edgecommons.panels.v2",
      provider: identity.component,
      renderer: "descriptor",
      ...(defaultView !== undefined ? { defaultView } : {}),
      views,
    };
    return {
      schemaVersion: "edgecommons.component.describe.v1",
      component: identity.toObject(),
      digest: digestDescribePayload(commands, panels),
      commands,
      panels,
    };
  }

  /**
   * Builds the two own-inbox wildcards (D-U28: `ecv1/{device}/{component}/+/cmd/#` instance-scope
   * and `ecv1/{device}/{component}/cmd/#` component-scope, through the topic builder under this
   * component's identity + root mode) and subscribes both on the PRIMARY connection. Startup is
   * single-flight and bounded: `ACTIVE` is published only after the selected transport has
   * acknowledged both filters. A failed attempt remains `FAILED` with a sanitized error and any
   * late acknowledgement is immediately unsubscribed.
   *
   * @param timeoutMs bounded acknowledgement wait (default 10 seconds)
   */
  start(timeoutMs = CommandInbox.DEFAULT_START_TIMEOUT_MS): Promise<void> {
    if (!Number.isInteger(timeoutMs) || timeoutMs < 1 || timeoutMs > 300_000) {
      throw new Error("command inbox start timeoutMs must be an integer between 1 and 300000");
    }
    if (this.started || this.closed || this.inboxState === CommandInboxState.Failed) {
      return Promise.resolve();
    }
    if (this.startPromise !== undefined) {
      return this.startPromise;
    }

    const attempt = this.startAttempt(timeoutMs);
    const tracked = attempt.finally(() => {
      if (this.startPromise === tracked) this.startPromise = undefined;
    });
    this.startPromise = tracked;
    return tracked;
  }

  private async startAttempt(timeoutMs: number): Promise<void> {
    this.transition(CommandInboxState.Starting);
    let filter: string | undefined;
    let componentFilter: string | undefined;
    try {
      const config = this.configProvider();
      const identity = config.componentIdentity;
      const uns = new Uns(identity, config.topicIncludeRoot);
      // Pin every scope token to this component's own identity: the site value is consulted
      // only under an effective root mode (D-U25 makes it a no-op otherwise).
      const site = identity.hier.length >= 2 ? identity.hier[0].value : undefined;
      const scope = {
        site,
        device: identity.device,
        component: identity.component,
        instance: identity.instance,
      };
      // D-U28: the component identity is component-scoped (no instance), so a plain filter renders
      // the instance slot as '+' (instance-scope: .../+/cmd/#); the component-scope filter omits
      // the slot (.../cmd/#). Subscribe BOTH. Assigned BEFORE subscribing so a delivery racing the
      // subscribe call is retained until the acknowledgement transitions this inbox to ACTIVE.
      filter = uns.filter(UnsClass.Cmd, scope);
      componentFilter = uns.filter(UnsClass.Cmd, scope, false);
      this.inboxFilter = filter;
      this.componentInboxFilter = componentFilter;
      const sub1 = this.messaging.subscribe(filter, (topic, message) => this.receiveDuringActivation(topic, message));
      const sub2 = this.messaging.subscribe(componentFilter, (topic, message) =>
        this.receiveDuringActivation(topic, message),
      );
      // A transport call cannot be cancelled through the public messaging interface. If either
      // eventually succeeds after this start attempt timed out or close() won the race, remove
      // the late subscription without allowing it to resurrect the inbox state.
      const cleanLate = (f: string) => (): void => {
        if (this.closed || this.inboxState !== CommandInboxState.Starting) {
          void this.messaging.unsubscribe(f).catch(() => undefined);
        }
      };
      void sub1.then(cleanLate(filter), () => undefined);
      void sub2.then(cleanLate(componentFilter), () => undefined);
      await awaitSubscriptionAcknowledgement(sub1, timeoutMs);
      await awaitSubscriptionAcknowledgement(sub2, timeoutMs);
      if (this.closed || this.inboxState !== CommandInboxState.Starting) {
        await this.messaging.unsubscribe(filter).catch(() => undefined);
        await this.messaging.unsubscribe(componentFilter).catch(() => undefined);
        this.activationPending = [];
        return;
      }
      this.started = true;
      this.transition(CommandInboxState.Active);
      this.drainActivationPending();
      logger.info(
        `command inbox subscribed on '${filter}' and '${componentFilter}' (verbs: ${[...this.verbs()].join(", ")})`,
      );
    } catch (e) {
      // Be defensive about providers that complete subscription work before reporting a local
      // failure: a failed start must never leave a partially live command plane behind.
      if (filter) await this.messaging.unsubscribe(filter).catch(() => undefined);
      if (componentFilter) await this.messaging.unsubscribe(componentFilter).catch(() => undefined);
      this.started = false;
      this.activationPending = [];
      if (this.closed) {
        return;
      }
      this.transition(CommandInboxState.Failed, "COMMAND_INBOX_SUBSCRIPTION_FAILED");
      logger.warn("command inbox failed to start; command plane is unavailable");
      logger.debug(`command inbox startup detail: ${errMsg(e)}`);
    }
  }

  /** Retain startup-racing deliveries; only ACTIVE inboxes may dispatch command work. */
  private receiveDuringActivation(topic: string, message: Message): void | Promise<void> {
    if (this.closed) return;
    if (this.inboxState === CommandInboxState.Starting) {
      if (this.activationPending.length >= CommandInbox.MAX_PENDING_STARTUP_DELIVERIES) {
        logger.warn("command inbox dropped a delivery while startup acknowledgement was pending");
        return;
      }
      this.activationPending.push({ topic, message });
      return;
    }
    if (this.inboxState === CommandInboxState.Active) {
      return this.handle(topic, message);
    }
  }

  /** Dispatch retained deliveries only after the ACTIVE state is externally visible. */
  private drainActivationPending(): void {
    const pending = this.activationPending;
    this.activationPending = [];
    for (const { topic, message } of pending) {
      void this.handle(topic, message);
    }
  }

  /**
   * One received `cmd` envelope: extract the verb from the topic, validate the envelope
   * (`header.name` must equal the verb), dispatch, reply. Never throws — a malformed or
   * foreign payload is ignored at DEBUG.
   */
  private async handle(topic: string, message: Message): Promise<void> {
    try {
      if (this.closed || this.inboxState !== CommandInboxState.Active) {
        return;
      }
      // D-U28: the instance slot is optional, so a command arrives on either
      // ".../{instance}/cmd/{verb}" or ".../cmd/{verb}". Locate the "/cmd/" class marker and take
      // the verb after it — unambiguous for both scopes (an instance is never a class token).
      if (!topic) {
        return;
      }
      const cmdMarker = topic.indexOf("/cmd/");
      if (cmdMarker < 0) {
        // ".../cmd/#" also matches the bare ".../cmd" parent level - nothing to dispatch.
        logger.debug(`ignoring cmd delivery without a '/cmd/' segment: '${topic}'`);
        return;
      }
      const verb = topic.slice(cmdMarker + 5); // 5 = "/cmd/".length
      if (verb === "") {
        return;
      }
      if (CommandInbox.DELEGATED_VERBS.has(verb)) {
        logger.debug(`ignoring delegated verb '${verb}' (owned by another library subscription)`);
        return;
      }
      if (!message || !message.header || message.header.name !== verb) {
        // Malformed/foreign: never replied to (a reply would race foreign conventions using a
        // different header name on a cmd topic), never a crash.
        logger.debug(`ignoring malformed/foreign cmd payload on '${topic}' (header.name must equal the topic verb)`);
        return;
      }
      await this.dispatch(verb, message);
    } catch (e) {
      logger.debug(`ignoring malformed cmd payload on '${topic}': ${errMsg(e)}`);
    }
  }

  /** Dispatches a well-formed request to its handler and replies (when `reply_to` set). */
  private async dispatch(verb: string, request: Message): Promise<void> {
    const replyTo = request.getReplyTo();
    const wantsReply = replyTo !== undefined && replyTo !== "";
    const outcomeHandler = this.outcomeHandlers.get(verb);
    const handler = this.handlers.get(verb);
    if (!handler && !outcomeHandler) {
      if (wantsReply) {
        logger.debug(`unknown verb '${verb}' - sending ${CommandInbox.ERR_UNKNOWN_VERB} error reply`);
        await this.sendReply(
          request,
          verb,
          errorBody(CommandInbox.ERR_UNKNOWN_VERB, `verb '${verb}' is not registered on this component`),
        );
      } else {
        logger.debug(`ignoring unknown fire-and-forget verb '${verb}'`);
      }
      return;
    }
    if (outcomeHandler) {
      await this.dispatchOutcome(verb, request, wantsReply, outcomeHandler);
      return;
    }
    let result: CommandResult;
    try {
      result = await handler!(request);
    } catch (e) {
      if (e instanceof CommandException) {
        if (wantsReply) {
          await this.sendReply(request, verb, errorBody(e.code, e.message));
        } else {
          logger.warn(`fire-and-forget verb '${verb}' failed (${e.code}): ${e.message}`);
        }
      } else if (wantsReply) {
        await this.sendReply(request, verb, errorBody(CommandInbox.ERR_HANDLER_ERROR, errMsg(e)));
      } else {
        logger.warn(`fire-and-forget verb '${verb}' failed: ${errMsg(e)}`);
      }
      return;
    }
    if (wantsReply) {
      await this.sendReply(request, verb, successBody(result));
    }
  }

  private async dispatchOutcome(
    verb: string,
    request: Message,
    wantsReply: boolean,
    handler: OutcomeCommandHandler,
  ): Promise<void> {
    let outcome: CommandOutcome;
    try {
      outcome = await handler(request);
      if (!outcome || typeof outcome !== "object" || !("kind" in outcome)) {
        throw new Error("outcome handler returned an invalid outcome");
      }
    } catch (e) {
      if (e instanceof CommandException) {
        await this.handleOutcomeError(verb, request, wantsReply, e.code, e.message);
      } else {
        await this.handleOutcomeError(verb, request, wantsReply, CommandInbox.ERR_HANDLER_ERROR, errMsg(e));
      }
      return;
    }

    switch (outcome.kind) {
      case "immediateSuccess":
        if (wantsReply) await this.sendReply(request, verb, successBody(outcome.result));
        return;
      case "immediateError":
        await this.handleOutcomeError(verb, request, wantsReply, outcome.code, outcome.message);
        return;
      case "deferred":
        if (outcome.token instanceof DeferredReply && outcome.token._validFor(this, request, verb)) {
          if (outcome.postAcceptContinuation !== undefined) {
            // Legacy deferred outcomes may already be settling when they return. A continuation
            // is stricter: it can start only after this inbox accepts a currently OPEN token.
            if (outcome.token.state() !== DeferredReplyState.Open) {
              await this.handleOutcomeError(
                verb,
                request,
                wantsReply,
                CommandInbox.ERR_HANDLER_ERROR,
                "post-accept continuation requires an open deferred token",
              );
              return;
            }
            this.startPostAcceptContinuation(outcome.token, outcome.postAcceptContinuation);
          }
          // Returning now releases the normal subscription-dispatch concurrency permit; the job
          // lifetime is represented only by the bounded deferred registry.
          return;
        }
        if (outcome.token instanceof DeferredReply) outcome.token._discardIfProvisional(this);
        await this.handleOutcomeError(
          verb,
          request,
          wantsReply,
          CommandInbox.ERR_HANDLER_ERROR,
          "handler returned an invalid, inactive, or foreign deferred token",
        );
        return;
      default:
        await this.handleOutcomeError(
          verb,
          request,
          wantsReply,
          CommandInbox.ERR_HANDLER_ERROR,
          "handler returned an unknown command outcome",
        );
    }
  }

  /**
   * Schedules application work only after the dispatcher accepted the exact OPEN token. The
   * small bound prevents command traffic from becoming unbounded in-memory work; rejected or
   * failed continuations settle through the ordinary guarded error path instead of leaking an
   * open deferred reply.
   */
  private startPostAcceptContinuation(token: DeferredReply, continuation: PostAcceptContinuation): void {
    if (this.closed || this.postAcceptOutstanding >= CommandInbox.MAX_POST_ACCEPT_CONTINUATIONS) {
      logger.warn("post-accept deferred continuation capacity exhausted");
      token.settleError(
        CommandInbox.ERR_HANDLER_ERROR,
        "the deferred command continuation could not be started",
      );
      return;
    }

    this.postAcceptOutstanding++;
    void Promise.resolve()
      .then(continuation)
      .catch((error: unknown) => {
        logger.warn(`post-accept deferred continuation failed: ${errMsg(error)}`);
        token.settleError(
          CommandInbox.ERR_HANDLER_ERROR,
          "the deferred command continuation failed",
        );
      })
      .finally(() => {
        this.postAcceptOutstanding--;
      });
  }

  private async handleOutcomeError(
    verb: string,
    request: Message,
    wantsReply: boolean,
    code: string,
    message: string,
  ): Promise<void> {
    if (wantsReply) {
      await this.sendReply(request, verb, errorBody(code, message));
    } else {
      logger.warn(`fire-and-forget outcome verb '${verb}' failed (${code}): ${message}`);
    }
  }

  /** @internal */
  _activateDeferred(entry: DeferredEntry): boolean {
    if (entry.state !== DeferredReplyState.Provisional) return false;
    entry.state = DeferredReplyState.Open;
    entry.activated = true;
    return true;
  }

  /** @internal */
  _discardDeferred(entry: DeferredEntry): boolean {
    if (entry.state !== DeferredReplyState.Provisional) return false;
    entry.state = DeferredReplyState.Discarded;
    this.deferredDiscarded++;
    this.cleanupDeferred(entry);
    return true;
  }

  /** @internal */
  _settleDeferred(entry: DeferredEntry, body: Record<string, unknown>): SettlementResult {
    if (entry.state !== DeferredReplyState.Open) return settlementResultFor(entry.state);
    const reply = MessageBuilder.create(entry.verb, CommandInbox.CMD_MESSAGE_VERSION)
      .withCommand(cloneCommandValue(body) as Record<string, unknown>)
      .withConfig(this.configProvider())
      .build();
    // JavaScript executes this check+transition synchronously on one event-loop turn, providing
    // the CAS-style single winner before any await/yield point.
    if (entry.state !== DeferredReplyState.Open) return settlementResultFor(entry.state);
    entry.state = DeferredReplyState.Settling;
    entry.reply = reply;
    this.scheduleDeferredAttempt(entry, 0);
    return SettlementResult.Accepted;
  }

  private scheduleDeferredAttempt(entry: DeferredEntry, delayMs: number): void {
    if (entry.state !== DeferredReplyState.Settling) return;
    entry.retryTimer = setTimeout(() => void this.publishDeferredAttempt(entry), Math.max(0, delayMs));
    if (typeof entry.retryTimer.unref === "function") entry.retryTimer.unref();
  }

  private async publishDeferredAttempt(entry: DeferredEntry): Promise<void> {
    if (entry.state !== DeferredReplyState.Settling || !entry.reply) return;
    let remaining = entry.expiresAtMs - performance.now();
    if (remaining <= 0) {
      this.expireSettlingDeferred(entry);
      return;
    }
    // `performance.now()` is fractional milliseconds, while the strict confirmed-publish
    // contract accepts only a positive integer timeout. Flooring preserves the lifetime bound;
    // if less than one whole millisecond remains, a valid confirmation timeout cannot be issued
    // without exceeding that bound, so expire instead of extending it to one millisecond.
    const attemptTimeout = Math.floor(Math.min(CommandInbox.DEFERRED_ATTEMPT_TIMEOUT_MS, remaining));
    if (attemptTimeout < 1) {
      this.expireSettlingDeferred(entry);
      return;
    }
    entry.attempts++;
    try {
      const confirmed = this.messaging.replyConfirmed;
      if (typeof confirmed !== "function") {
        throw new PublishConfirmationError("unsupported", "messaging service does not support confirmed reply");
      }
      await confirmed.call(this.messaging, entry.requestMetadata, entry.reply, attemptTimeout);
      if (entry.state === DeferredReplyState.Settling) {
        entry.state = DeferredReplyState.Settled;
        this.deferredSettled++;
        this.cleanupDeferred(entry);
      }
    } catch (e) {
      if (entry.state !== DeferredReplyState.Settling) return;
      remaining = entry.expiresAtMs - performance.now();
      if (remaining <= 0) {
        this.expireSettlingDeferred(entry);
        return;
      }
      const retryMs = Math.min(
        CommandInbox.DEFERRED_RETRY_MAX_MS,
        CommandInbox.DEFERRED_RETRY_INITIAL_MS * 2 ** Math.min(10, entry.attempts - 1),
        Math.max(1, remaining),
      );
      logger.debug(
        `deferred reply attempt ${entry.attempts} for verb '${entry.verb}' failed; retrying in ${retryMs} ms: ${errMsg(e)}`,
      );
      this.scheduleDeferredAttempt(entry, retryMs);
    }
  }

  private expireDeferred(entry: DeferredEntry): void {
    if (entry.state === DeferredReplyState.Provisional) {
      entry.state = DeferredReplyState.Expired;
      this.deferredExpired++;
      this.cleanupDeferred(entry);
    } else if (entry.state === DeferredReplyState.Open) {
      entry.state = DeferredReplyState.Expired;
      this.recordOpenExpiration(entry);
    }
    // SETTLING owns the final result: its strict publish timeout is bounded by expiresAtMs.
  }

  private expireSettlingDeferred(entry: DeferredEntry): void {
    if (entry.state !== DeferredReplyState.Settling) return;
    entry.state = DeferredReplyState.Expired;
    this.recordOpenExpiration(entry);
  }

  private recordOpenExpiration(entry: DeferredEntry): void {
    this.deferredExpired++;
    this.deferredOpenExpired++;
    logger.warn(
      `DEFERRED_REPLY_EXPIRED: open deferred reply for verb '${entry.verb}' expired after ${entry.attempts} confirmed publication attempt(s)`,
    );
    this.cleanupDeferred(entry);
  }

  private cancelDeferredOnShutdown(entry: DeferredEntry): void {
    if (isDeferredTerminal(entry.state)) return;
    entry.state = DeferredReplyState.CancelledOnShutdown;
    this.deferredCancelledOnShutdown++;
    this.cleanupDeferred(entry);
  }

  private cleanupDeferred(entry: DeferredEntry): void {
    if (entry.cleaned) return;
    entry.cleaned = true;
    this.deferredEntries.delete(entry.id);
    if (entry.expirationTimer) clearTimeout(entry.expirationTimer);
    if (entry.retryTimer) clearTimeout(entry.retryTimer);
  }

  /**
   * Publishes a reply to the request's `reply_to` through the existing reply mechanism (the
   * messaging service stamps the request's `correlation_id` onto the reply). The reply is
   * config-stamped, so it carries the responder's `identity` (+ `tags`). Best-effort: a failing
   * reply (e.g. a hostile reserved-class `reply_to` rejected by the guard) is logged and
   * swallowed.
   */
  private async sendReply(request: Message, verb: string, body: Record<string, unknown>): Promise<void> {
    try {
      const reply = MessageBuilder.create(verb, CommandInbox.CMD_MESSAGE_VERSION)
        .withCommand(body)
        .withConfig(this.configProvider())
        .build();
      await this.messaging.reply(request, reply);
    } catch (e) {
      logger.warn(`command reply for verb '${verb}' failed: ${errMsg(e)}`);
    }
  }

  /**
   * Stops the inbox: unsubscribes the inbox wildcard (while messaging is still up — the
   * unsubscribe-before-exit rule) and stops dispatching. Idempotent.
   */
  async close(): Promise<void> {
    if (this.closed) {
      return;
    }
    this.closed = true;
    this.activationPending = [];

    const stopping: Promise<void>[] = [];
    for (const entry of [...this.deferredEntries.values()]) {
      if (entry.state === DeferredReplyState.Open) {
        entry.state = DeferredReplyState.Settling;
        stopping.push(this.attemptStoppingReply(entry));
      } else {
        this.cancelDeferredOnShutdown(entry);
      }
    }
    if (stopping.length > 0) {
      let shutdownTimer: NodeJS.Timeout | undefined;
      try {
        await Promise.race([
          Promise.allSettled(stopping),
          new Promise<void>((resolve) => {
            shutdownTimer = setTimeout(resolve, CommandInbox.DEFERRED_SHUTDOWN_TIMEOUT_MS);
          }),
        ]);
      } finally {
        if (shutdownTimer) clearTimeout(shutdownTimer);
      }
    }
    for (const entry of [...this.deferredEntries.values()]) {
      this.cancelDeferredOnShutdown(entry);
    }

    for (const f of [this.inboxFilter, this.componentInboxFilter]) {
      if (f) {
        try {
          await this.messaging.unsubscribe(f);
        } catch (e) {
          logger.debug(`command-inbox unsubscribe of '${f}' failed: ${errMsg(e)}`);
        }
      }
    }
    this.started = false;
    this.transition(CommandInboxState.Stopped);
  }

  private async attemptStoppingReply(entry: DeferredEntry): Promise<void> {
    try {
      const confirmed = this.messaging.replyConfirmed;
      if (typeof confirmed !== "function") {
        throw new PublishConfirmationError("unsupported", "messaging service does not support confirmed reply");
      }
      const remaining = entry.expiresAtMs - performance.now();
      const attemptTimeout = Math.floor(
        Math.min(CommandInbox.DEFERRED_SHUTDOWN_TIMEOUT_MS, remaining),
      );
      if (attemptTimeout >= 1) {
        const reply = MessageBuilder.create(entry.verb, CommandInbox.CMD_MESSAGE_VERSION)
          .withCommand(
            errorBody(
              CommandInbox.ERR_COMPONENT_STOPPING,
              "the component stopped before the deferred command could reply",
            ),
          )
          .withConfig(this.configProvider())
          .build();
        await confirmed.call(
          this.messaging,
          entry.requestMetadata,
          reply,
          attemptTimeout,
        );
      }
    } catch (e) {
      logger.debug(`deferred COMPONENT_STOPPING reply for verb '${entry.verb}' failed: ${errMsg(e)}`);
    } finally {
      this.cancelDeferredOnShutdown(entry);
    }
  }
}
