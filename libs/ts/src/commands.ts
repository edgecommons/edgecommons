/**
 * The library-owned component **command inbox** — the minimal `commands()` facade
 * (DESIGN-uns §7.3 / §9.5, the edge-console slice S2): every component subscribes, on its
 * PRIMARY (local/IPC) connection, its own `main`-instance command-inbox wildcard
 *
 * ```text
 *   ecv1/{device}/{component}/main/cmd/#
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
 *   `ecv1/{device}/config/main/cmd/get-configuration` rendezvous where a component fetches its
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
 * initialization completes; {@link CommandInbox.close} unsubscribes the inbox (before messaging
 * closes — the unsubscribe-before-exit rule). Only the `main`-instance inbox is subscribed in
 * this slice; per-instance inboxes ride the full `commands()` facade (Phase 5).
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

import type { Config } from "./config/model";
import type { InstanceConnectivity } from "./instance_connectivity";
import { logger } from "./logging";
import type { Message } from "./message";
import { MessageBuilder } from "./message";
import type { IMessagingService } from "./messaging/types";
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
  /** panel id -> descriptor; panel descriptors are carried verbatim for `describe`. */
  private readonly panelViews = new Map<string, Record<string, unknown>>();
  /** The subscribed inbox filter (`…/cmd/#`); undefined until {@link start} builds it. */
  private inboxFilter?: string;
  /** The filter minus the trailing `#` — the verb-extraction prefix (`…/cmd/`). */
  private inboxPrefix?: string;
  private started = false;
  private closed = false;

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
    for (const token of verb.split("/")) {
      checkToken(token, "verb token");
    }
    if (CommandInbox.BUILT_IN_VERBS.has(verb)) {
      throw new Error(`verb '${verb}' is a built-in verb and cannot be shadowed`);
    }
    if (CommandInbox.DELEGATED_VERBS.has(verb)) {
      throw new Error(`verb '${verb}' is owned by another library subsystem and cannot be registered`);
    }
    if (this.handlers.has(verb)) {
      throw new Error(`verb '${verb}' is already registered - unregister it first to replace the handler`);
    }
    this.handlers.set(verb, handler);
    logger.debug(`command verb '${verb}' registered`);
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
    if (this.handlers.delete(verb)) {
      logger.debug(`command verb '${verb}' unregistered`);
    }
  }

  /** The currently registered verbs (built-ins + custom) — a snapshot copy. */
  verbs(): Set<string> {
    return new Set(this.handlers.keys());
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
    const commands = [...this.handlers.keys()].sort().map((verb) => ({
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
   * Builds the own-inbox wildcard (`ecv1/{device}/{component}/main/cmd/#`, through the topic
   * builder under this component's identity + root mode) and subscribes it on the PRIMARY
   * connection. Best-effort and idempotent: on any subscription failure the inbox logs a WARN
   * and disables itself; the component must come up regardless.
   */
  async start(): Promise<void> {
    if (this.started || this.closed) {
      return;
    }
    try {
      const config = this.configProvider();
      const identity = config.componentIdentity;
      const uns = new Uns(identity, config.topicIncludeRoot);
      // Pin every scope token to this component's own identity: the site value is consulted
      // only under an effective root mode (D-U25 makes it a no-op otherwise).
      const site = identity.hier.length >= 2 ? identity.hier[0].value : undefined;
      const filter = uns.filter(UnsClass.Cmd, {
        site,
        device: identity.device,
        component: identity.component,
        instance: identity.instance,
      });
      this.inboxFilter = filter;
      // ".../cmd/#" -> ".../cmd/" - the verb is the topic's remainder after this prefix.
      // Assigned BEFORE subscribing so a delivery racing the subscribe call sees it.
      this.inboxPrefix = filter.slice(0, filter.length - 1);
      await this.messaging.subscribe(filter, (topic, message) => this.handle(topic, message));
      this.started = true;
      logger.info(`command inbox subscribed on '${filter}' (verbs: ${[...this.handlers.keys()].join(", ")})`);
    } catch (e) {
      logger.warn(`failed to start the command inbox (continuing without it): ${errMsg(e)}`);
    }
  }

  /**
   * One received `cmd` envelope: extract the verb from the topic, validate the envelope
   * (`header.name` must equal the verb), dispatch, reply. Never throws — a malformed or
   * foreign payload is ignored at DEBUG.
   */
  private async handle(topic: string, message: Message): Promise<void> {
    try {
      if (this.closed) {
        return;
      }
      if (!this.inboxPrefix || !topic || !topic.startsWith(this.inboxPrefix)) {
        // ".../cmd/#" also matches the bare ".../cmd" parent level - nothing to dispatch.
        logger.debug(`ignoring cmd delivery outside the inbox prefix: '${topic}'`);
        return;
      }
      const verb = topic.slice(this.inboxPrefix.length);
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
    const handler = this.handlers.get(verb);
    if (!handler) {
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
    let result: CommandResult;
    try {
      result = await handler(request);
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
      const body = { ok: true, result: result ?? {} };
      await this.sendReply(request, verb, body);
    }
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
    if (this.started && this.inboxFilter) {
      try {
        await this.messaging.unsubscribe(this.inboxFilter);
      } catch (e) {
        logger.debug(`command-inbox unsubscribe of '${this.inboxFilter}' failed: ${errMsg(e)}`);
      }
    }
  }
}
