/**
 * The library-owned `cfg` publisher (UNS-CANONICAL-DESIGN §4.3): announces the component's
 * effective (redacted) configuration on `ecv1[/{site}]/{device}/{component}/main/cfg` — once at
 * startup (after initialization completes) and again on every configuration change. The body is
 * `{"config": <effective config, redacted>}`; the `cfg` class is reserved (§4.1), so the publish
 * goes through the privileged reserved-publish seam. (This is the push half only — the
 * `republish-cfg` pull verb lands in a later phase.)
 *
 * **Redaction v1** (§4.3): `$secret` references are never resolved (the raw config is published
 * as-is, so a `{"$secret": …}` ref stays a ref); every value under a `credentials` key inside
 * the top-level `messaging` section, and every value of a key named `password` or `pin`
 * (case-insensitive) anywhere, is replaced with `"***"`.
 */
import { logger } from "../logging";
import { MessageBuilder } from "../message";
import type { IMessagingService } from "../messaging/types";
import { publishReservedVia } from "../messaging/service";
import { Uns, UnsClass } from "../uns";
import type { Config } from "./model";
import type { ConfigurationChangeListener } from "./index";

/** The cfg announcement's envelope header name (§4.3). */
const CFG_MESSAGE_NAME = "cfg";
const CFG_MESSAGE_VERSION = "1.0";
/** The redaction placeholder. */
export const REDACTED = "***";

/**
 * Publishes the effective (redacted) configuration on the UNS `cfg` topic. Register it as a
 * configuration-change listener (each hot reload republishes) and call {@link publishNow} for
 * the startup announcement.
 */
export class EffectiveConfigPublisher implements ConfigurationChangeListener {
  constructor(
    private readonly configProvider: () => Config,
    private readonly messaging: IMessagingService,
  ) {}

  /**
   * Publishes the effective (redacted) configuration to the component's UNS `cfg` topic.
   * Best-effort: any failure is logged and swallowed — a cfg announcement must never crash the
   * component.
   */
  async publishNow(): Promise<void> {
    try {
      const config = this.configProvider();
      const topic = new Uns(config.componentIdentity, config.topicIncludeRoot).topic(UnsClass.Cfg);
      const body = { config: redact(config.raw) };
      const cfgMessage = MessageBuilder.create(CFG_MESSAGE_NAME, CFG_MESSAGE_VERSION)
        .withPayload(body)
        .withConfig(config)
        .build();
      await publishReservedVia(this.messaging, topic, cfgMessage);
      logger.debug(`published effective (redacted) configuration on '${topic}'`);
    } catch (e) {
      logger.warn(`effective-config publish failed: ${e instanceof Error ? e.message : String(e)}`);
    }
  }

  async onConfigurationChange(_config: Config): Promise<boolean> {
    await this.publishNow();
    return true;
  }

  /**
   * The current effective configuration, redacted (redaction v1) — the single snapshot source
   * shared by the `cfg` push (this publisher) and the `get-configuration` command verb's reply
   * (DESIGN-uns §9.5 Flow B), so both surfaces always agree byte-for-byte. Unlike the Java
   * `redactedEffectiveConfig()`, this never returns `undefined`: the TS `Config` snapshot is
   * always resolved (fail-fast at construction, UNS-CANONICAL-DESIGN §1.5), so there is no
   * mock/test bring-up state with no effective configuration.
   */
  redactedEffectiveConfig(): Record<string, unknown> {
    return redact(this.configProvider().raw);
  }
}

/**
 * Redaction v1 (§4.3) over a deep copy of the effective config: every value of a key named
 * `password` or `pin` (case-insensitive, anywhere) and every value of a `credentials` key at
 * any depth inside the top-level `messaging` section becomes {@link REDACTED}. `$secret` refs
 * are untouched (they are never resolved here, so no secret material exists to leak).
 *
 * @param config the effective config (not mutated)
 * @returns the redacted deep copy
 */
export function redact(config: Record<string, unknown>): Record<string, unknown> {
  return redactObject(config, false, true) as Record<string, unknown>;
}

/**
 * Recursive redaction walk over a copy. `inMessaging` is true anywhere under the **top-level**
 * `messaging` section (the `messaging.*.credentials` rule); `topLevel` is true only for the
 * config root, so a nested `messaging` key elsewhere does not trigger the credentials rule.
 */
function redactObject(obj: Record<string, unknown>, inMessaging: boolean, topLevel: boolean): unknown {
  const out: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(obj)) {
    if (
      key.toLowerCase() === "password" ||
      key.toLowerCase() === "pin" ||
      (inMessaging && key.toLowerCase() === "credentials")
    ) {
      out[key] = REDACTED;
      continue;
    }
    const nextInMessaging = inMessaging || (topLevel && key === "messaging");
    if (value !== null && typeof value === "object" && !Array.isArray(value)) {
      out[key] = redactObject(value as Record<string, unknown>, nextInMessaging, false);
    } else if (Array.isArray(value)) {
      // Mirror Java: array items that are objects recurse; everything else passes through.
      out[key] = value.map((item) =>
        item !== null && typeof item === "object" && !Array.isArray(item)
          ? redactObject(item as Record<string, unknown>, inMessaging, false)
          : item,
      );
    } else {
      out[key] = value;
    }
  }
  return out;
}
