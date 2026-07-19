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
 *
 * ## The command seam
 *
 * On top of `readSignals`/`writeSignal`, a session also serves the `sb/*` command surface
 * (`src/commands.ts`): {@link DeviceSession.readNamed} (an on-demand read of named signals) and
 * {@link DeviceSession.browse} (paged address-space discovery). Both have sensible defaults on
 * {@link BaseDeviceSession} — `readNamed` reads everything and filters, `browse` reports
 * {@link BrowseError.unsupported} — so a protocol with a fixed register map and no discovery stays
 * honest without extra code. The backend's {@link DeviceBackend.inventory} answers `sb/signals`
 * from config, with **no** device round-trip.
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
 * One signal in the adapter's inventory — its stable id and human label, known from
 * config/backend **without a device round-trip**. Backs the `sb/signals` command.
 */
export interface SignalInfo {
  /** The canonical, stable id (the `sb/read`/`sb/write` `signalId`). */
  readonly id: string;
  /** A human label, when the backend has one. */
  readonly name?: string;
}

/**
 * One entry discovered by {@link DeviceSession.browse} — a signal the device *offers*, whether or
 * not it is configured. Backs the `sb/browse` diagnostics surface.
 */
export interface BrowsedSignal {
  /** The stable id a consumer would configure or read. */
  readonly id: string;
  /** A human label, when the device provides one. */
  readonly name?: string;
  /** The device-native type, kept verbatim for diagnosis (`"REAL"`, `"holding/uint16"`, …). */
  readonly typeName: string;
}

/**
 * One page of a {@link DeviceSession.browse} enumeration. Browsing is **paged** because a device's
 * address space can be large; `nextCursor` is set while more pages remain.
 */
export interface BrowsePage {
  readonly entries: readonly BrowsedSignal[];
  /** Opaque continuation token; pass it back as the next `cursor`. Absent on the last page. */
  readonly nextCursor?: string;
}

/**
 * Why a `sb/browse` could not answer. Kept distinct from {@link DeviceError} because "this protocol
 * has no discovery" is a permanent, honest capability limit — not a link failure. A session rejects
 * its {@link DeviceSession.browse} with one of these; the command layer maps `UNSUPPORTED` to
 * `BROWSE_UNSUPPORTED` and `FAILED` to `BROWSE_FAILED`.
 */
export class BrowseError extends Error {
  private constructor(
    /** `UNSUPPORTED` (no discovery service) or `FAILED` (a mid-browse link/protocol error). */
    readonly reason: "UNSUPPORTED" | "FAILED",
    message: string,
  ) {
    super(message);
    this.name = "BrowseError";
  }

  /**
   * The protocol has no discovery service. The default seam impl rejects with this, so an adapter
   * that cannot browse stays honest (the command maps it to `BROWSE_UNSUPPORTED`).
   */
  static unsupported(): BrowseError {
    return new BrowseError("UNSUPPORTED", "this adapter has no discovery service");
  }

  /** A mid-browse failure (a link error, a malformed reply). Maps to `BROWSE_FAILED`. */
  static failed(message: string): BrowseError {
    return new BrowseError("FAILED", message);
  }

  static isBrowseError(e: unknown): e is BrowseError {
    return e instanceof BrowseError;
  }
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

  /**
   * Read a named subset **now** (backs `sb/read`). The {@link BaseDeviceSession} default reads
   * everything and filters, which is correct for any backend; override it when your protocol can
   * read a subset more cheaply. Reject only when the *connection* is broken.
   */
  readNamed(ids: readonly string[]): Promise<Reading[]>;

  /** Write a value back to the device. Rejects if the write is refused or the link is down. */
  writeSignal(signalId: string, value: unknown): Promise<void>;

  /**
   * Enumerate the device's address space, one page at a time (backs `sb/browse`). The
   * {@link BaseDeviceSession} default rejects with {@link BrowseError.unsupported} — a protocol
   * with no discovery (Modbus, a fixed register map) is honest to leave it unimplemented. Override
   * it when your protocol can enumerate (OPC UA browse, an EtherNet/IP tag list). Rejects with a
   * {@link BrowseError} (`UNSUPPORTED` / `FAILED`).
   */
  browse(cursor: string | undefined, max: number): Promise<BrowsePage>;

  /** Close the connection. Must be safe to call twice. */
  close(): Promise<void>;
}

/**
 * A base session supplying the default seam behavior — `readNamed` reads-all-and-filters, `browse`
 * reports {@link BrowseError.unsupported}, `close` is a no-op. Extend it and implement only the two
 * required methods; TypeScript interfaces cannot carry default methods, so this abstract class is
 * how the "default trait impl" of the Rust/Java/Python seams is expressed here.
 */
export abstract class BaseDeviceSession implements DeviceSession {
  abstract readSignals(): Promise<Reading[]>;
  abstract writeSignal(signalId: string, value: unknown): Promise<void>;

  async readNamed(ids: readonly string[]): Promise<Reading[]> {
    const all = await this.readSignals();
    return all.filter((r) => ids.includes(r.signalId));
  }

  async browse(_cursor: string | undefined, _max: number): Promise<BrowsePage> {
    throw BrowseError.unsupported();
  }

  async close(): Promise<void> {}
}

/** Opens sessions. One factory per protocol. */
export interface DeviceBackend {
  /** The protocol's name, as it appears in config and in the published `device.adapter` field. */
  readonly kind: string;

  /**
   * The signal inventory this backend exposes for a device, **without connecting** — read from
   * config in a real adapter. Backs `sb/signals` (a config view, no device round-trip). The
   * simulator returns a fixed pair so the command has something to show. Optional: a backend that
   * cannot list its inventory offline reports an empty list.
   */
  inventory?(cfg: ConnectionConfig): SignalInfo[];

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

/**
 * The signals the simulator exposes — the ids it reads and the one it fails. A real backend derives
 * this from config; the simulator hard-codes it so `sb/signals` and `sb/browse` have content.
 */
const SIM_SIGNALS: ReadonlyArray<{ id: string; name: string; type: string }> = [
  { id: "temperature-1", name: "Ambient temperature", type: "REAL" },
  { id: "pressure-1", name: "Line pressure", type: "REAL" },
];

export class SimBackend implements DeviceBackend {
  readonly kind = "sim";

  inventory(_cfg: ConnectionConfig): SignalInfo[] {
    return SIM_SIGNALS.map((s) => ({ id: s.id, name: s.name }));
  }

  async connect(cfg: ConnectionConfig): Promise<DeviceSession> {
    if (!cfg.endpoint) {
      // A missing endpoint will never fix itself: permanent, so the supervisor does not spend the
      // next hour reconnecting to nothing.
      throw DeviceError.permanent("no endpoint configured");
    }
    return new SimSession();
  }
}

export class SimSession extends BaseDeviceSession {
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

  /**
   * A one-page browse of the simulator's inventory. A real backend pages a large address space and
   * returns a `nextCursor`; the simulator has two signals, so the first page is the last page.
   */
  async browse(cursor: string | undefined, _max: number): Promise<BrowsePage> {
    // A cursor means "the page after the last one" — the sim has nothing more.
    if (cursor !== undefined) return { entries: [] };
    return {
      entries: SIM_SIGNALS.map((s) => ({ id: s.id, name: s.name, typeName: s.type })),
    };
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
