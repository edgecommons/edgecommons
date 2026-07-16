/**
 * Configuration source — CONFIG_COMPONENT.
 *
 * Loads (and hot-reloads) configuration from a dedicated configuration-manager component over
 * the UNS config rendezvous (UNS-CANONICAL-DESIGN §4.3, D-U19 **Flow A**).
 *
 * Wire contract (a convention shared with the config server, matching Java/Python/Rust
 * verbatim):
 * - **Flow A — GET**: a request to `ecv1/{device}/config/cmd/get-configuration` (component scope,
 *   D-U28; with `{device}` = the sanitized resolved thing name). `config` is a **reserved-by-convention
 *   logical component name** — the config server is the sole subscriber and replies via
 *   `reply_to` with the configuration as the message body. Because this request runs during
 *   config bootstrap — *before* the {@link Config} snapshot (and therefore the component
 *   identity) exists — it carries no envelope identity; the requester **self-identifies in the
 *   body** with `{"component": "<short name>"}` (§1.5).
 * - **set-config push**: the server pushes a fire-and-forget `cmd` (no `reply_to` — a
 *   notification-style command) to the component's own inbox
 *   `ecv1/{device}/{component}/cmd/set-config` (component scope, D-U28); the body is a lineage
 *   bundle, delivered to the watch callback.
 *
 * The topics are minted locally from the resolved thing name and component name handed to the
 * constructor (the same inputs the config model later uses), both passed through the normative
 * UNS token sanitizer — never from a `Config`/`Uns` (identity is not resolved yet). These are
 * `cmd`-class topics — not library-reserved — so they publish through the ordinary messaging
 * surface and pass the reserved-topic guard.
 *
 * `load` sends a `GetConfiguration` v1.0 request and awaits the reply (30 s per attempt — the
 * pre-config built-in deadline, §5 — up to 3 attempts), returning the reply body for the
 * layered coordinator to parse as a lineage bundle; it throws a
 * {@link EdgeCommonsError} of kind `Config` after 3 failures. `watch` subscribes to the set-config inbox
 * and forwards each message body.
 */
import { EdgeCommonsError } from "../../errors";
import { IMessagingService } from "../../messaging/types";
import { MessageBuilder, Message } from "../../message";
import { sanitize } from "../template";
import { ConfigSource, ConfigWatch } from "./index";

/**
 * Flow-A GET request topic (§4.3): the config server's rendezvous under the
 * reserved-by-convention logical component name `config`, component scope (no instance, D-U28).
 */
const GET_TOPIC_TEMPLATE = "ecv1/{device}/config/cmd/get-configuration";
/**
 * The pushed `set-config` command's topic — this component's OWN inbox (§4.3): the
 * server-to-component push replacing the legacy `.../updated` subscription (component scope, D-U28).
 */
const SET_CONFIG_TOPIC_TEMPLATE = "ecv1/{device}/{component}/cmd/set-config";
/** Per-attempt reply timeout (ms) — the pre-config built-in request deadline (§5). */
const REPLY_TIMEOUT_MS = 30_000;
/** Maximum request attempts before giving up. */
const MAX_ATTEMPTS = 3;

/** Substitute the pre-sanitized `{device}` / `{component}` tokens into a topic template. */
function mintTopic(template: string, deviceToken: string, componentToken: string): string {
  return template.replace("{device}", deviceToken).replace("{component}", componentToken);
}

/**
 * Reduces a component name to its short form (the segment after the last `.`), the existing
 * `{ComponentName}` semantics (D-U18).
 */
function shortComponentName(componentName: string): string {
  return componentName.includes(".")
    ? componentName.slice(componentName.lastIndexOf(".") + 1)
    : componentName;
}

/** Messaging-backed configuration-component source (UNS Flow A). */
export class ConfigComponentSource implements ConfigSource {
  private readonly getTopic: string;
  private readonly setConfigTopic: string;
  /** The sanitized short component name — the body self-identification token (§1.5). */
  private readonly componentToken: string;

  constructor(
    private readonly messaging: IMessagingService,
    thingName: string,
    componentName: string,
  ) {
    // Mint the UNS tokens locally (no Config/Uns dependency — identity is not resolved yet):
    // device = sanitized resolved thing name, component = sanitized short name, mirroring the
    // {ThingName}/{ComponentName} template semantics and §1.5 steps 4-5.
    const deviceToken = sanitize(thingName);
    this.componentToken = sanitize(shortComponentName(componentName));
    this.getTopic = mintTopic(GET_TOPIC_TEMPLATE, deviceToken, this.componentToken);
    this.setConfigTopic = mintTopic(SET_CONFIG_TOPIC_TEMPLATE, deviceToken, this.componentToken);
  }

  async load(): Promise<unknown> {
    let lastErr = "no attempts made";
    for (let attempt = 1; attempt <= MAX_ATTEMPTS; attempt++) {
      // The requester self-identifies in the BODY (§1.5): during bootstrap there is no
      // resolved identity, so the envelope carries no identity element — the config server
      // routes on {"component"} instead.
      const request = MessageBuilder.create("GetConfiguration", "1.0")
        .withPayload({ component: this.componentToken })
        .build();
      try {
        const reply = await this.messaging.request(this.getTopic, request, REPLY_TIMEOUT_MS);
        return reply.getBody();
      } catch (e) {
        lastErr = (e as Error).message;
        console.warn(
          `config component request failed (attempt ${attempt}, topic ${this.getTopic}); retrying: ${lastErr}`,
        );
      }
    }
    throw EdgeCommonsError.config(
      `failed to load configuration from the config component after ${MAX_ATTEMPTS} attempts: ${lastErr}`,
    );
  }

  sourceName(): string {
    return "CONFIG_COMPONENT";
  }

  async watch(onUpdate: (raw: unknown) => void): Promise<ConfigWatch | undefined> {
    await this.messaging.subscribe(
      this.setConfigTopic,
      (_topic: string, msg: Message) => {
        onUpdate(msg.getBody());
      },
      16,
      1,
    );
    return {
      close: async () => {
        await this.messaging.unsubscribe(this.setConfigTopic);
      },
    };
  }
}
