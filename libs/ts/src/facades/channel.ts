/**
 * A publish-channel address (DESIGN-class-facades §4, `DESIGN-channels.md`): the uniform
 * `{ local, northbound, stream:<name> }` routing target the publish facades resolve on.
 *
 * - `LOCAL` — the local/IPC bus (`messaging().publish`). The default.
 * - `NORTHBOUND` — AWS IoT Core (`messaging().publishToIoTCore`).
 * - `stream(name)` — the named durable telemetry stream (`getStreams().stream(name).append(...)`
 *   via the {@link StreamSink} seam); **only {@link DataFacade} honors it** —
 *   `events()`/`app()` reject a stream channel (they are low-rate control-plane, not bulk
 *   telemetry).
 *
 * Modeled as a discriminated union (not a bare string enum) because the `stream` variant
 * carries a stream name — mirrors the `UnsScope` interface-plus-namesake-const idiom already
 * used in this library (`uns.ts`) rather than the Java value class's `equals`/`hashCode`
 * boilerplate (TS/vitest compares structurally with `toEqual` out of the box).
 */
import { GgError } from "../errors";

/** The routing kind — the discriminant of the {@link Channel} union. */
export type ChannelKind = "local" | "northbound" | "stream";

/** The local/IPC bus channel. */
export interface LocalChannel {
  readonly kind: "local";
}

/** The AWS IoT Core (northbound) channel. */
export interface NorthboundChannel {
  readonly kind: "northbound";
}

/** The named-durable-stream channel. */
export interface StreamChannel {
  readonly kind: "stream";
  readonly streamName: string;
}

/** A publish-channel address — one of {@link LocalChannel}/{@link NorthboundChannel}/{@link StreamChannel}. */
export type Channel = LocalChannel | NorthboundChannel | StreamChannel;

/**
 * Factory/parse namespace for {@link Channel} (mirrors the `UnsScope` const-namesake idiom):
 * `Channel.LOCAL`, `Channel.NORTHBOUND`, `Channel.stream(name)`, `Channel.fromConfig(value)`.
 */
export const Channel = {
  /** The local/IPC bus channel (the default). */
  LOCAL: { kind: "local" } as LocalChannel,

  /** The AWS IoT Core (northbound) channel. */
  NORTHBOUND: { kind: "northbound" } as NorthboundChannel,

  /**
   * The named-durable-stream channel.
   *
   * @throws GgError (kind `Validation`) when `name` is empty
   */
  stream(name: string): StreamChannel {
    if (!name) {
      throw GgError.validation("stream channel name must be non-empty");
    }
    return { kind: "stream", streamName: name };
  },

  /**
   * Parses a config `publish.channel` string into a {@link Channel} (DESIGN-class-facades §4,
   * Option C). Recognized: `"local"` → {@link Channel.LOCAL}; `"northbound"` / `"iotcore"` /
   * `"iot_core"` → {@link Channel.NORTHBOUND}; `"stream:<name>"` → {@link Channel.stream}. Any
   * other (or null/empty/absent) value yields `undefined` so the caller can fall through to its
   * own default.
   */
  fromConfig(value: string | undefined | null): Channel | undefined {
    if (value === undefined || value === null) {
      return undefined;
    }
    const v = value.trim();
    if (v === "") {
      return undefined;
    }
    const lower = v.toLowerCase();
    if (lower === "local") {
      return Channel.LOCAL;
    }
    if (lower === "northbound" || lower === "iotcore" || lower === "iot_core") {
      return Channel.NORTHBOUND;
    }
    if (lower.startsWith("stream:")) {
      const name = v.slice("stream:".length);
      return name === "" ? undefined : Channel.stream(name);
    }
    return undefined;
  },

  /** `"local"` / `"northbound"` / `"stream:<name>"` — the config-string form (round-trips {@link fromConfig}). */
  toConfigString(channel: Channel): string {
    switch (channel.kind) {
      case "local":
        return "local";
      case "northbound":
        return "northbound";
      case "stream":
        return `stream:${channel.streamName}`;
    }
  },
};
