/**
 * # The device seam: what a *protocol adapter* talks to
 *
 * {@link DeviceSession} is one live connection to one device. Implement it once per protocol —
 * Modbus, OPC UA, whatever you are bridging — and everything above it (the connection lifecycle,
 * backoff, publishing, health) is written against the interface and never learns your protocol.
 *
 * **The boundary rule, and it is worth enforcing in review:** a backend knows protocols. It does
 * **not** know EdgeCommons topics, the UNS, message envelopes, or metrics. This module imports
 * nothing from `@edgecommons/edgecommons` — deliberately. If your `DeviceSession` starts importing
 * the UNS/messaging modules, the seam has leaked.
 *
 * ## Signals, not tags
 *
 * A **signal** is one data point — a measured value with identity, quality, and timestamps.
 * (OPC UA calls it a "tag"; Modbus calls it a "register".) The word "tag" is reserved in
 * EdgeCommons for the envelope's *business metadata*, which is a different thing entirely.
 *
 * ## Quality is not optional
 *
 * Every sample carries a `quality` normalized to `GOOD | BAD | UNCERTAIN`, plus the native code in
 * `qualityRaw` for diagnosis. This is what lets a consumer gate on quality without knowing your
 * protocol — and it is why a read failure must be published as a `BAD` sample rather than
 * swallowed. A signal that silently stops updating is indistinguishable from one that is simply
 * not changing.
 */

/**
 * Normalized quality. The protocol's own status code goes in {@link Reading.qualityRaw}.
 *
 * Declared here rather than imported from the library on purpose: the backend seam must not depend
 * on EdgeCommons. `src/app.ts` maps these onto the library's `Quality` when it publishes.
 *
 * `Uncertain` is unused by the simulated backend and used constantly by real ones: a stale cached
 * read, a value outside its calibrated range, a sensor that answered but warned.
 */
export enum Quality {
  Good = "GOOD",
  Bad = "BAD",
  Uncertain = "UNCERTAIN",
}

/** One reading from the device. */
export interface Reading {
  /** The canonical, stable id the rest of the fleet keys on (e.g. `ns=3;i=1001`). */
  readonly signalId: string;
  /** A human label. */
  readonly name?: string;
  readonly value: unknown;
  readonly quality: Quality;
  /** The protocol-native status code, kept verbatim for diagnosis. */
  readonly qualityRaw?: string;
}

/**
 * Why talking to the device failed — and, crucially, whether reconnecting could help.
 *
 * * **Transient:** the link is down, or the device is busy. Reconnect and retry.
 * * **Permanent:** misconfiguration — a bad endpoint, a rejected credential, an address that does
 *   not exist. Reconnecting will fail identically, so the supervisor backs off hard rather than
 *   hammering a device that is never going to answer.
 */
export class DeviceError extends Error {
  constructor(
    message: string,
    /** Whether retrying could ever help. */
    readonly transient: boolean,
  ) {
    super(message);
    this.name = "DeviceError";
  }

  static transientError(message: string): DeviceError {
    return new DeviceError(message, true);
  }

  static permanent(message: string): DeviceError {
    return new DeviceError(message, false);
  }

  /** Whether a failure is worth retrying (`false` for a misconfiguration). */
  static isTransient(e: unknown): boolean {
    return e instanceof DeviceError && e.transient;
  }
}

/**
 * How to reach one device. Deliberately open (`additionalProperties: true` in the schema): every
 * protocol needs different keys, and this is the one place the adapter should not be strict.
 */
export interface ConnectionConfig {
  /** The endpoint, in whatever form the protocol uses. Published in `device.endpoint`. */
  readonly endpoint: string;
  /** Everything else the protocol needs: a unit id, a security policy, a slave address. */
  readonly [key: string]: unknown;
}

/** A live connection to one device. **This is the interface you implement.** */
export interface DeviceSession {
  /**
   * Read the configured signals once.
   *
   * A read that fails for *one* signal must return that signal with {@link Quality.Bad} rather
   * than failing the whole call — one dead register must not blind you to the other ninety-nine.
   * Reject only when the *connection* is broken.
   */
  readSignals(): Promise<Reading[]>;

  /** Write a value back to the device. Rejects if the write is refused or the link is down. */
  writeSignal(signalId: string, value: unknown): Promise<void>;

  /** Close the connection. Must be safe to call twice. */
  close(): Promise<void>;
}

/** Opens sessions. One factory per protocol. */
export interface DeviceBackend {
  /** The protocol's name, as it appears in config and in the published `device.adapter` field. */
  readonly kind: string;

  /**
   * Connect to one device. Rejects with a transient {@link DeviceError} when the device is
   * unreachable, and a permanent one when the configuration is wrong.
   */
  connect(cfg: ConnectionConfig): Promise<DeviceSession>;
}

// --- The simulated backend -------------------------------------------------------------------
//
// A real adapter replaces this with its protocol. It ships so the component runs with no hardware,
// and so the tests have something to talk to — and a backend you can run on a laptop is worth more
// than one you can only run next to a PLC.

export class SimBackend implements DeviceBackend {
  readonly kind = "sim";

  async connect(cfg: ConnectionConfig): Promise<DeviceSession> {
    if (!cfg.endpoint) {
      // A missing endpoint will never fix itself: permanent, so the supervisor does not spend the
      // next hour reconnecting to nothing.
      throw DeviceError.permanent("no endpoint configured");
    }
    return new SimSession();
  }
}

export class SimSession implements DeviceSession {
  private tick = 0;
  private closed = false;

  async readSignals(): Promise<Reading[]> {
    this.tick += 1;
    const value = 20.0 + 5.0 * Math.sin(this.tick / 10.0);
    return [
      {
        signalId: "temperature-1",
        name: "Ambient temperature",
        value,
        quality: Quality.Good,
        qualityRaw: "OK",
      },
      // A signal the simulated device cannot currently read. It is published as BAD rather than
      // omitted, because "I could not read this" is information and silence is not.
      {
        signalId: "pressure-1",
        name: "Line pressure",
        value: null,
        quality: Quality.Bad,
        qualityRaw: "SENSOR_FAULT",
      },
    ];
  }

  async writeSignal(signalId: string, value: unknown): Promise<void> {
    // eslint-disable-next-line no-console
    console.log(`sim: write accepted signal=${signalId} value=${JSON.stringify(value)}`);
  }

  async close(): Promise<void> {
    this.closed = true;
  }

  /** Exposed for the tests: `close()` is idempotent. */
  isClosed(): boolean {
    return this.closed;
  }
}

/** The backends this component understands. Add yours here as you implement it. */
export function backendFor(kind: string): DeviceBackend | undefined {
  return kind === "sim" ? new SimBackend() : undefined;
}
