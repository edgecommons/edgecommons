/**
 * Configuration source — CONFIG_COMPONENT.
 *
 * Loads (and hot-reloads) configuration from a dedicated configuration-manager
 * component via request/reply over messaging. A port of Rust `config_component.rs`.
 *
 * Transport-agnostic: it works over whichever {@link IMessagingService} the runtime
 * wired (Greengrass IPC in GREENGRASS mode, dual-broker MQTT in STANDALONE mode). The
 * topic contract matches the Java/Python/Rust libraries verbatim (cross-language
 * parity):
 * - request: `ggcommons/{ThingName}/config/get/{ComponentName}`
 * - updated: `ggcommons/{ThingName}/config/{ComponentName}/updated`
 *
 * `load` sends a `GetConfiguration` v1.0 request and awaits the reply (30s timeout,
 * up to 3 attempts), returning the reply body; it throws a {@link GgError} of kind
 * `Config` after 3 failures. `watch` subscribes to the updated topic and forwards
 * each message body.
 */
import { GgError } from "../../errors";
import { IMessagingService } from "../../messaging/types";
import { MessageBuilder, Message } from "../../message";
import { ConfigSource, ConfigWatch } from "./index";

/** Request-topic template (parity with Java/Python/Rust). */
const GET_TOPIC_TEMPLATE = "ggcommons/{ThingName}/config/get/{ComponentName}";
/** Updated-topic template (parity with Java/Python/Rust). */
const UPDATED_TOPIC_TEMPLATE = "ggcommons/{ThingName}/config/{ComponentName}/updated";
/** Per-attempt reply timeout (ms). */
const REPLY_TIMEOUT_MS = 30_000;
/** Maximum request attempts before giving up. */
const MAX_ATTEMPTS = 3;

/** Substitute `{ThingName}` / `{ComponentName}` into a topic template. */
function resolveTopic(template: string, thing: string, component: string): string {
  return template.replace("{ThingName}", thing).replace("{ComponentName}", component);
}

/** Messaging-backed configuration-component source. */
export class ConfigComponentSource implements ConfigSource {
  private readonly thingName: string;
  private readonly getTopic: string;
  private readonly updatedTopic: string;

  constructor(
    private readonly messaging: IMessagingService,
    thingName: string,
    componentName: string,
  ) {
    this.thingName = thingName;
    this.getTopic = resolveTopic(GET_TOPIC_TEMPLATE, thingName, componentName);
    this.updatedTopic = resolveTopic(UPDATED_TOPIC_TEMPLATE, thingName, componentName);
  }

  async load(): Promise<unknown> {
    let lastErr = "no attempts made";
    for (let attempt = 1; attempt <= MAX_ATTEMPTS; attempt++) {
      const request = MessageBuilder.create("GetConfiguration", "1.0")
        .withThingName(this.thingName)
        .withPayload({})
        .build();
      try {
        const reply = await this.messaging.request(this.getTopic, request, REPLY_TIMEOUT_MS);
        return reply.getBody();
      } catch (e) {
        lastErr = (e as Error).message;
        console.warn(`config component request failed (attempt ${attempt}, topic ${this.getTopic}); retrying: ${lastErr}`);
      }
    }
    throw GgError.config(
      `failed to load configuration from the config component after ${MAX_ATTEMPTS} attempts: ${lastErr}`,
    );
  }

  sourceName(): string {
    return "CONFIG_COMPONENT";
  }

  async watch(onUpdate: (raw: unknown) => void): Promise<ConfigWatch | undefined> {
    await this.messaging.subscribe(
      this.updatedTopic,
      (_topic: string, msg: Message) => {
        onUpdate(msg.getBody());
      },
      16,
      1,
    );
    return {
      close: async () => {
        await this.messaging.unsubscribe(this.updatedTopic);
      },
    };
  }
}
