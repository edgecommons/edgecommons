/**
 * The constructed `SouthboundSignalUpdate` body value (DESIGN-class-facades §2.1,
 * `docs/SOUTHBOUND.md` §2) — the value object that **replaces the adapters' hand-assembled**
 * JSON object. Mirrors the Java `SignalUpdate`/`SignalUpdate.Sample`/`SignalUpdate.Builder`.
 *
 * **TS-idiom divergence from Java:** Java telescopes `addSample(value)` /
 * `addSample(value, quality)` / `addSample(value, quality, sourceTs)` / `addSample(Sample)` as
 * four overloads. A `Sample` is a plain shape-matching object in TS (no nominal `record` type),
 * so overloading on "is the first arg a value or a full Sample" would be genuinely ambiguous
 * (a signal's `value` can itself be a plain object, e.g. a waveform/vector reading). Instead
 * {@link SignalUpdateBuilder.addSample} takes the value plus an options object
 * ({@link SampleOptions}) covering `quality`/`qualityRaw`/`sourceTs`/`serverTs` in one
 * unambiguous call; {@link SignalUpdateBuilder.addSamples} remains the batch/coalesced path.
 */
import { GgError } from "../errors";
import type { Channel } from "./channel";
import type { Quality } from "./quality";

/**
 * One sample: a measured `value` (REQUIRED at publish) plus the optional quality/timestamp
 * parts. An omitted `quality` is defaulted to `Quality.Good` by {@link DataFacade}; an omitted
 * `serverTs` is filled with now; `sourceTs` is never synthesized; `qualityRaw` is the synthetic
 * `"unspecified"` marker when (and only when) the quality was defaulted, else passed through
 * verbatim.
 */
export interface Sample {
  /** The measured value (JSON-native: number/boolean/string/array/object) — REQUIRED at publish. */
  readonly value: unknown;
  /** The normalized quality, or `undefined` to default to {@link Quality.Good}. */
  readonly quality?: Quality;
  /** The native status code, or `undefined`. */
  readonly qualityRaw?: string;
  /** The device/field ISO-8601 timestamp, or `undefined` (never synthesized). */
  readonly sourceTs?: string;
  /** The protocol-server ISO-8601 timestamp, or `undefined` to default to now. */
  readonly serverTs?: string;
}

/** The optional parts of a {@link Sample} beyond its required `value` (see {@link SignalUpdateBuilder.addSample}). */
export interface SampleOptions {
  quality?: Quality;
  qualityRaw?: string;
  sourceTs?: string;
  serverTs?: string;
}

/**
 * The immutable, built signal update — the value {@link DataFacade.publish}/{@link DataFacade.buildBody}
 * consume. Obtain one from {@link SignalUpdateBuilder.build}.
 */
export interface SignalUpdate {
  /** The optional `device` block (`{adapter, instance, endpoint}`), or `undefined`. */
  readonly device?: Record<string, unknown>;
  /** The stable `signal.id` (REQUIRED at publish; the consumer key). */
  readonly signalId?: string;
  /** The human `signal.name`, or `undefined`. */
  readonly signalName?: string;
  /** The protocol-native `signal.address`, or `undefined`. */
  readonly signalAddress?: Record<string, unknown>;
  /** The samples (may be empty — {@link DataFacade.publish} rejects an empty list). */
  readonly samples: readonly Sample[];
  /** The channel path (the `data/{signalPath}` tail); `undefined` means "use signalId". */
  readonly signalPath?: string;
  /** The per-call {@link Channel} override, or `undefined` (resolve config default ▸ LOCAL). */
  readonly via?: Channel;
}

/** The effective channel path: {@link SignalUpdate.signalPath} when set, else {@link SignalUpdate.signalId}. */
export function effectiveSignalPath(update: SignalUpdate): string | undefined {
  return update.signalPath ?? update.signalId;
}

/**
 * The fluent `SouthboundSignalUpdate` builder — `signal(id).name(n).address(a).device(...)
 * .addSample(value, opts).signalPath(p).publish()`. Reused across all four languages (mirrors
 * the Java `SignalUpdate.Builder`).
 *
 * A builder obtained from {@link DataFacade.signal} is **facade-bound**: {@link publish}
 * publishes through that facade. A detached builder (constructed directly, e.g. by the
 * `uns-test-vectors` conformance harness) has no bound facade — call {@link build} and pass the
 * result to {@link DataFacade.publish} instead; calling {@link publish} on a detached builder
 * throws.
 */
export class SignalUpdateBuilder {
  private deviceValue?: Record<string, unknown>;
  private signalNameValue?: string;
  private signalAddressValue?: Record<string, unknown>;
  private readonly samplesValue: Sample[] = [];
  private signalPathValue?: string;
  private viaValue?: Channel;

  /**
   * @param signalId  the stable `signal.id` (REQUIRED at publish — the consumer key)
   * @param publisher the facade-bound publish callback (set by {@link DataFacade.signal}), or
   *                  `undefined` for a detached builder (terminate with {@link build} instead)
   */
  constructor(
    private readonly signalId: string | undefined,
    private readonly publisher?: (update: SignalUpdate) => Promise<void>,
  ) {}

  /** Sets the human `signal.name`. */
  name(name: string): this {
    this.signalNameValue = name;
    return this;
  }

  /** Sets the protocol-native `signal.address`. */
  address(address: Record<string, unknown>): this {
    this.signalAddressValue = address;
    return this;
  }

  /** Sets the `device` block from its three parts (any may be `undefined`). */
  device(adapter: string | undefined, instance: string | undefined, endpoint: string | undefined): this;
  /** Sets a pre-built `device` block. */
  device(device: Record<string, unknown>): this;
  device(a: string | Record<string, unknown> | undefined, instance?: string, endpoint?: string): this {
    if (typeof a === "object" && a !== null) {
      this.deviceValue = a;
      return this;
    }
    const d: Record<string, unknown> = {};
    if (a !== undefined) d.adapter = a;
    if (instance !== undefined) d.instance = instance;
    if (endpoint !== undefined) d.endpoint = endpoint;
    this.deviceValue = d;
    return this;
  }

  /**
   * Appends one sample: `value` (REQUIRED at publish) plus the optional quality/timestamp parts
   * (see the class doc for why this collapses Java's four `addSample` overloads into one).
   */
  addSample(value: unknown, opts?: SampleOptions): this {
    this.samplesValue.push({
      value,
      quality: opts?.quality,
      qualityRaw: opts?.qualityRaw,
      sourceTs: opts?.sourceTs,
      serverTs: opts?.serverTs,
    });
    return this;
  }

  /** Appends a batch of fully-specified samples (the coalesced-publish path). */
  addSamples(samples: readonly Sample[]): this {
    this.samplesValue.push(...samples);
    return this;
  }

  /**
   * Sets the channel path — the `data/{signalPath}` tail (each `/`-separated token is sanitized
   * into a UNS token by the facade). When unset, the stable `signalId` is used as the path
   * (D-U15's sanitized-path-vs-stable-id split still holds — the body's raw id rides untouched).
   */
  signalPath(signalPath: string): this {
    this.signalPathValue = signalPath;
    return this;
  }

  /** Sets a per-call {@link Channel} override (LOCAL / NORTHBOUND / stream). */
  via(channel: Channel): this {
    this.viaValue = channel;
    return this;
  }

  /** Builds the immutable {@link SignalUpdate} (for the `DataFacade.publish(update)` form). */
  build(): SignalUpdate {
    return {
      device: this.deviceValue,
      signalId: this.signalId,
      signalName: this.signalNameValue,
      signalAddress: this.signalAddressValue,
      samples: [...this.samplesValue],
      signalPath: this.signalPathValue,
      via: this.viaValue,
    };
  }

  /**
   * Builds and publishes through the originating facade.
   *
   * @throws GgError (kind `Validation`) when this builder was created detached (no facade) —
   *                 use {@link build} + `DataFacade.publish(update)` instead
   */
  async publish(): Promise<void> {
    if (!this.publisher) {
      throw GgError.validation(
        "this SignalUpdateBuilder is detached - call build() and pass it to DataFacade.publish(update)",
      );
    }
    await this.publisher(this.build());
  }
}
