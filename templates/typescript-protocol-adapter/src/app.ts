/**
 * # <<COMPONENTNAME>> — a southbound protocol adapter
 *
 * An **adapter** connects to devices, reads signals, and publishes them onto the UNS in the shape
 * the rest of the fleet expects — so that a consumer can chart a Modbus register and an OPC UA node
 * without knowing either protocol.
 *
 * ```text
 *   connect ──► poll ──► publish SouthboundSignalUpdate ──► report health
 *      ▲                                                         │
 *      └──────────── reconnect with backoff ◄────────────────────┘
 * ```
 *
 * One loop per instance: an instance is one device, and its connection lifecycle is its own.
 *
 * ## The contract you are implementing (docs/SOUTHBOUND.md)
 *
 * * Publish `SouthboundSignalUpdate` on the `data` class, **via the `data()` facade** — never
 *   hand-build the body and never hand-write the topic. The facade constructs
 *   `{device, signal, samples}`, mints `ecv1/{device}/{component}/{instance}/data/{signal}`, and
 *   stamps identity. A hand-rolled topic is a topic that will disagree with the envelope.
 * * **Quality on every sample**, normalized to `GOOD | BAD | UNCERTAIN`, with the native code in
 *   `qualityRaw`.
 * * Emit **`southbound_health`**, dimensioned by instance, so an operator can see a link go down
 *   without reading logs.
 * * Report **per-instance connectivity** ({@link connectivityOf}), so the fleet sees which devices
 *   this adapter is actually talking to — pushed on every `state` keepalive and returned by the
 *   built-in `status` verb, from one provider.
 * * Serve **read/write commands** — and allow-list the writes. An adapter that will write any
 *   address it is asked to is a control-system vulnerability, not a feature.
 */
import {
  CommandException,
  Config,
  ConfigurationChangeListener,
  DataFacade,
  EdgeCommons,
  EventsFacade,
  InstanceConnectivity,
  MetricBuilder,
  MetricService,
  Quality as LibQuality,
  Severity,
  logger,
} from "@edgecommons/edgecommons";

import {
  ConnectionConfig,
  DeviceBackend,
  DeviceError,
  DeviceSession,
  Quality,
  Reading,
  backendFor,
} from "./device";

/** The metric every southbound adapter emits (SOUTHBOUND.md §5). */
export const HEALTH_METRIC = "southbound_health";
/** The write verb this adapter serves. `/`-namespaced verbs are allowed by the command inbox. */
export const WRITE_VERB = "sb/write";

/** How often the health metric is re-emitted while a device is polling, in ms. */
const HEALTH_INTERVAL_MS = 60_000;

// --- config ----------------------------------------------------------------------------------

/**
 * Writes are **allow-listed by stable `signal.id`**. An empty list means this adapter is
 * read-only, which is the correct default for anything touching a control system.
 */
export class Writes {
  constructor(readonly allow: readonly string[] = []) {}

  permits(signalId: string): boolean {
    return this.allow.includes(signalId);
  }
}

/** One device == one entry of `component.instances[]`. */
export interface DeviceConfig {
  /**
   * The instance id. It is the `{instance}` token of this device's UNS topics, so it must be a
   * valid UNS token (lower-kebab).
   */
  readonly id: string;
  /** Which backend to use. Matches {@link DeviceBackend.kind}. */
  readonly adapter: string;
  readonly connection: ConnectionConfig;
  /** How often to read, in milliseconds. */
  readonly pollIntervalMs: number;
  readonly writes: Writes;
}

const DEVICE_KEYS = new Set(["id", "adapter", "connection", "pollIntervalMs", "writes"]);
const DEFAULT_POLL_MS = 5_000;

/**
 * Parse one entry of `component.instances[]`.
 *
 * Unknown keys are rejected rather than ignored: a typo'd key is a mistake, not a no-op. (The
 * `connection` object is the one exception — see {@link ConnectionConfig}.)
 *
 * @throws Error when the entry is malformed
 */
export function parseDevice(raw: unknown): DeviceConfig {
  if (typeof raw !== "object" || raw === null) throw new Error("a device must be an object");
  const o = raw as Record<string, unknown>;

  for (const key of Object.keys(o)) {
    if (!DEVICE_KEYS.has(key)) throw new Error(`unknown key '${key}'`);
  }
  if (typeof o.id !== "string" || o.id === "") throw new Error("`id` is required");

  const connection = o.connection;
  if (typeof connection !== "object" || connection === null) {
    throw new Error("`connection` is required");
  }
  const endpoint = (connection as Record<string, unknown>).endpoint;
  if (typeof endpoint !== "string" || endpoint === "") {
    throw new Error("`connection.endpoint` is required");
  }

  const adapter = o.adapter === undefined ? "sim" : o.adapter;
  if (typeof adapter !== "string") throw new Error("`adapter` must be a string");

  const pollIntervalMs = o.pollIntervalMs === undefined ? DEFAULT_POLL_MS : o.pollIntervalMs;
  if (typeof pollIntervalMs !== "number" || pollIntervalMs < 1) {
    throw new Error("`pollIntervalMs` must be a positive number");
  }

  let writes = new Writes();
  if (o.writes !== undefined) {
    if (typeof o.writes !== "object" || o.writes === null) throw new Error("`writes` must be an object");
    const w = o.writes as Record<string, unknown>;
    for (const key of Object.keys(w)) {
      if (key !== "allow") throw new Error(`unknown key 'writes.${key}'`);
    }
    const allow = w.allow ?? [];
    if (!Array.isArray(allow) || allow.some((s) => typeof s !== "string")) {
      throw new Error("`writes.allow` must be an array of signal ids");
    }
    writes = new Writes(allow as string[]);
  }

  return {
    id: o.id,
    adapter,
    connection: connection as ConnectionConfig,
    pollIntervalMs,
    writes,
  };
}

// --- backoff ---------------------------------------------------------------------------------

/**
 * Reconnect backoff: exponential, capped, with **full jitter** — so a site whose PLC reboots does
 * not get every adapter in the plant reconnecting in lockstep on the same second.
 */
export class Backoff {
  constructor(
    readonly baseMs = 1_000,
    readonly maxMs = 60_000,
  ) {}

  /** A random delay in `[0, min(cap, base * 2^attempt))`. `rand01` is injected for the tests. */
  delayMs(attempt: number, rand01: number): number {
    const exp = this.baseMs * 2 ** Math.min(attempt, 20);
    const cap = Math.min(exp, this.maxMs);
    return Math.floor(Math.min(Math.max(rand01, 0), 1) * cap);
  }
}

// --- the write mailbox -----------------------------------------------------------------------

/** A write, on its way from the command inbox to the device's own loop. */
export interface WriteRequest {
  readonly signalId: string;
  readonly value: unknown;
  /** The device's answer. A write is confirmed, not fire-and-forget. */
  readonly settle: (error?: string) => void;
}

/**
 * A single-consumer mailbox with a deadline.
 *
 * A write cannot touch the session directly: the session lives in the device's own loop, and most
 * device protocols are a single request/response channel that would interleave into nonsense if a
 * write and a poll talked at once. So a write is *sent* to that loop, which serializes it against
 * the reads. This is why an adapter is one loop per device rather than a shared connection pool.
 */
export class Mailbox<T> {
  private readonly queue: T[] = [];
  private waiter?: () => void;

  send(item: T): void {
    this.queue.push(item);
    this.waiter?.();
  }

  /** Take the next item, waiting at most `timeoutMs`. Resolves `undefined` on timeout. */
  async receive(timeoutMs: number): Promise<T | undefined> {
    const first = this.queue.shift();
    if (first !== undefined) return first;
    if (timeoutMs <= 0) return undefined;

    await new Promise<void>((resolve) => {
      const timer = setTimeout(() => {
        this.waiter = undefined;
        resolve();
      }, timeoutMs);
      this.waiter = () => {
        clearTimeout(timer);
        this.waiter = undefined;
        resolve();
      };
    });
    return this.queue.shift();
  }
}

// --- health ----------------------------------------------------------------------------------

/**
 * This adapter's **own vocabulary** for a link's condition — what it reports as
 * `InstanceConnectivity.state`. A boolean cannot tell "has not connected yet" from "backing off
 * after a failure" from "configured with an adapter that does not exist"; an operator needs to, so
 * the richer token rides alongside the normalized flag.
 */
export type LinkState = "CONNECTING" | "ONLINE" | "BACKOFF" | "DISABLED";

/** The `southbound_health` measures, per instance (SOUTHBOUND.md §5), plus the link condition. */
export class Health {
  /** 1 = connected, 0 = down. */
  connectionState = 0;
  /**
   * The link's condition, in this adapter's vocabulary. A device starts CONNECTING: it is
   * configured and not yet reachable, which is exactly what must NOT look like a device nobody
   * configured at all.
   */
  link: LinkState = "CONNECTING";
  pollLatencyMs = 0;
  readErrors = 0;
  reconnects = 0;
  signalsPublished = 0;

  /**
   * Record the link's condition. The metric's boolean and the reported state token move
   * **together**, so the health dot and the label a console shows can never disagree.
   */
  setLink(state: LinkState): void {
    this.link = state;
    this.connectionState = state === "ONLINE" ? 1 : 0;
  }

  /** Snapshot the interval counters and reset them (a gauge stays, a counter resets). */
  takeInterval(): Record<string, number> {
    const values = {
      connectionState: this.connectionState,
      pollLatencyMs: this.pollLatencyMs,
      readErrors: this.readErrors,
      reconnects: this.reconnects,
      signalsPublished: this.signalsPublished,
    };
    this.readErrors = 0;
    this.reconnects = 0;
    this.signalsPublished = 0;
    return values;
  }
}

/**
 * One device's connectivity sample, for the provider registered in the {@link App} constructor.
 *
 * * `connected` is the **normalized** flag — always present, so a console renders a health dot for
 *   this adapter without knowing anything about its protocol.
 * * `state` is *this adapter's* vocabulary ({@link LinkState}) for the richer condition.
 * * `attributes` is the **open** bag: domain data only this adapter understands (here, which backend
 *   the device speaks), carried without destabilizing the two fields above that everyone reads.
 */
export function connectivityOf(cfg: DeviceConfig, health: Health): InstanceConnectivity {
  return InstanceConnectivity.of(cfg.id, health.link === "ONLINE", cfg.connection.endpoint)
    .withState(health.link)
    .withAttributes({ adapter: cfg.adapter });
}

// --- the southbound publish path ---------------------------------------------------------------

/** Map the backend's protocol-free quality onto the library's wire enum. */
export function toLibQuality(q: Quality): LibQuality {
  switch (q) {
    case Quality.Good:
      return LibQuality.Good;
    case Quality.Bad:
      return LibQuality.Bad;
    case Quality.Uncertain:
      return LibQuality.Uncertain;
  }
}

/**
 * Publish one poll's readings as `SouthboundSignalUpdate`s.
 *
 * The `data()` facade builds the body, mints the topic, and stamps identity. Do not hand-build any
 * of the three. **Every reading is published, including a failed one** — a `BAD` sample says "I
 * could not read this", and silence says nothing at all.
 */
export async function publishReadings(
  data: DataFacade,
  adapter: string,
  device: Pick<DeviceConfig, "id" | "connection">,
  readings: readonly Reading[],
  health?: Health,
): Promise<void> {
  for (const r of readings) {
    try {
      const signal = data.signal(r.signalId);
      if (r.name !== undefined) signal.name(r.name);
      await signal
        .device(adapter, device.id, device.connection.endpoint)
        .addSample(r.value, { quality: toLibQuality(r.quality), qualityRaw: r.qualityRaw })
        .publish();
      if (health) health.signalsPublished += 1;
    } catch (e) {
      logger.warn(`publish failed instance=${device.id} signal=${r.signalId}: ${String(e)}`);
    }
  }
}

// --- the write command -------------------------------------------------------------------------

/**
 * The `sb/write` handler.
 *
 * Scope rides an `instance` body field rather than a topic segment, so one inbox serves every
 * device this adapter owns. The allow-list is checked HERE, before the write ever reaches the
 * device: an adapter that writes whatever it is asked to is a control-system vulnerability, and
 * "the caller was authorized" is not this component's judgement to make.
 */
export async function handleWrite(
  devices: ReadonlyMap<string, DeviceConfig>,
  mailboxes: ReadonlyMap<string, Mailbox<WriteRequest>>,
  body: unknown,
): Promise<Record<string, unknown>> {
  const o = (typeof body === "object" && body !== null ? body : {}) as Record<string, unknown>;

  const instance = o.instance;
  if (typeof instance !== "string") throw new CommandException("BAD_ARGS", "expected `instance`");
  const signalId = o.signalId;
  if (typeof signalId !== "string") throw new CommandException("BAD_ARGS", "expected `signalId`");
  if (!("value" in o)) throw new CommandException("BAD_ARGS", "expected `value`");

  const cfg = devices.get(instance);
  if (!cfg) throw new CommandException("NO_SUCH_INSTANCE", instance);

  // THE ALLOW-LIST.
  if (!cfg.writes.permits(signalId)) {
    throw new CommandException(
      "WRITE_NOT_ALLOWED",
      `\`${signalId}\` is not in this instance's writes.allow list`,
    );
  }

  const mailbox = mailboxes.get(instance);
  if (!mailbox) throw new CommandException("DEVICE_UNAVAILABLE", "device loop is gone");

  // A write is CONFIRMED: the reply is the device's answer, not "we sent it".
  return new Promise<Record<string, unknown>>((resolve, reject) => {
    mailbox.send({
      signalId,
      value: o.value,
      settle: (error?: string) => {
        if (error === undefined) resolve({ written: signalId });
        else reject(new CommandException("WRITE_FAILED", error));
      },
    });
  });
}

// --- the app ---------------------------------------------------------------------------------

const sleep = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));
const rand01 = (): number => Math.random();

export class App {
  private readonly config: Config;
  private readonly metrics: MetricService;
  private readonly devices: DeviceConfig[] = [];
  private readonly mailboxes = new Map<string, Mailbox<WriteRequest>>();
  /** Each device's health: written by its own loop, read by the connectivity provider. */
  private readonly health = new Map<string, Health>();
  private readonly loops: Promise<void>[] = [];
  private stopped = false;

  constructor(private readonly gg: EdgeCommons) {
    this.config = gg.config();
    this.metrics = gg.metrics();

    const listener: ConfigurationChangeListener = {
      onConfigurationChange: (config: Config): boolean => {
        logger.info(`configuration changed (thing=${config.thingName})`);
        return true;
      },
    };
    gg.addConfigChangeListener(listener);

    // One device per instance. A malformed device is skipped with a warning rather than killing the
    // component — but if EVERY device is malformed there is nothing to run, and failing loudly
    // beats idling silently.
    for (const id of this.config.instanceIds()) {
      try {
        this.devices.push(parseDevice(this.config.instance(id)));
      } catch (e) {
        logger.warn(`skipping malformed device '${id}': ${String(e)}`);
      }
    }
    if (this.devices.length === 0) {
      throw new Error("no valid devices in component.instances[]");
    }

    // A device's health exists from the moment it is CONFIGURED, not from the moment its loop first
    // connects: a configured device that is down must never be indistinguishable from a device
    // nobody configured. Registered here, before run() starts a single worker, so the very first
    // keepalive already reports every device — as CONNECTING, connected=false.
    //
    // ONE provider, TWO surfaces: the library pushes this sample into the `state` keepalive's
    // `instances[]` every tick, and returns the very same sample from the built-in `status` verb
    // when a console asks. Whoever watches and whoever asks cannot get different answers. Keep it
    // cheap — it is sampled on the keepalive interval, and it reads only cached state.
    //
    // Reporting one entry per device is the whole point of the adapter archetype: the fleet sees
    // which of THIS component's devices are reachable, without minting a UNS instance per device.
    for (const device of this.devices) {
      this.health.set(device.id, new Health());
    }
    gg.setInstanceConnectivityProvider(() =>
      this.devices.map((d) => connectivityOf(d, this.health.get(d.id) as Health)),
    );
  }

  async run(): Promise<void> {
    for (const device of this.devices) {
      // Per-instance facades: data() mints THIS device's topics and stamps its identity.
      const instance = this.gg.instance(device.id);

      // The health metric is dimensioned BY INSTANCE, so a fleet view can show one device down
      // without averaging it away against the others.
      this.metrics.defineMetric(
        MetricBuilder.create(HEALTH_METRIC)
          .withConfig(this.config)
          .addMeasure("connectionState", "Count", 1)
          .addMeasure("pollLatencyMs", "Milliseconds", 1)
          .addMeasure("readErrors", "Count", 60)
          .addMeasure("reconnects", "Count", 60)
          .addMeasure("signalsPublished", "Count", 60)
          .addDimension("instance", device.id)
          .build(),
      );

      const mailbox = new Mailbox<WriteRequest>();
      this.mailboxes.set(device.id, mailbox);

      this.loops.push(
        this.runDevice(
          device,
          instance.data(),
          instance.events(),
          mailbox,
          this.health.get(device.id) as Health,
        ).catch((e: unknown) => logger.error(`device loop '${device.id}' stopped: ${String(e)}`)),
      );
    }

    // The southbound command surface. `ping` / `reload-config` / `get-configuration` are already
    // live — the library registered them before we ran. This one is the adapter's own.
    const commands = this.gg.commands();
    if (commands) {
      const devices = new Map(this.devices.map((d) => [d.id, d]));
      commands.register(WRITE_VERB, (request) => handleWrite(devices, this.mailboxes, request.body));
    }

    await Promise.all(this.loops);
  }

  /**
   * One device's lifecycle: connect, poll, publish, reconnect.
   *
   * The connect loop and the poll loop are nested on purpose. A read failure that breaks the link
   * drops out of the poll loop and back into connect — which is the only place that knows how to
   * back off. Retrying a read on a dead socket forever is the classic adapter bug.
   */
  private async runDevice(
    cfg: DeviceConfig,
    data: DataFacade,
    events: EventsFacade,
    mailbox: Mailbox<WriteRequest>,
    health: Health,
  ): Promise<void> {
    const backend: DeviceBackend | undefined = backendFor(cfg.adapter);
    if (!backend) {
      logger.error(`unknown adapter '${cfg.adapter}' for instance '${cfg.id}'`);
      // This device will never connect, and saying CONNECTING forever would be a lie no boolean
      // could correct. DISABLED is what a console needs to show, and why `state` exists.
      health.setLink("DISABLED");
      return;
    }

    const backoff = new Backoff();
    let attempt = 0;

    while (!this.stopped) {
      let session: DeviceSession;
      try {
        session = await backend.connect(cfg.connection);
      } catch (e) {
        // A permanent failure will fail identically forever, so back off to the ceiling
        // immediately rather than hammering a device that is never going to answer.
        const permanent = !DeviceError.isTransient(e);
        const wait = permanent ? backoff.maxMs : backoff.delayMs(attempt, rand01());
        logger.warn(
          `connect failed instance=${cfg.id} permanent=${permanent} waitMs=${wait}: ${String(e)}`,
        );
        health.setLink("BACKOFF");
        attempt += 1;
        await sleep(wait);
        continue;
      }

      attempt = 0;
      health.setLink("ONLINE");
      await this.emitHealth(health);
      await events
        .emit(Severity.Info, "device-connected", `connected to ${cfg.connection.endpoint}`, {
          instance: cfg.id,
          adapter: backend.kind,
        })
        .catch(() => undefined);
      // A raised alarm is cleared by the SAME type, so the pair rides one channel and a consumer
      // can match them.
      await events.clearAlarm("device-unreachable", { instance: cfg.id }).catch(() => undefined);

      await this.pollUntilDisconnected(cfg, session, data, backend.kind, mailbox, health);

      health.setLink("BACKOFF");
      health.reconnects += 1;
      await this.emitHealth(health);
      if (!this.stopped) {
        await events
          .raiseAlarm("device-unreachable", `lost the link to ${cfg.connection.endpoint}`, {
            instance: cfg.id,
          })
          .catch(() => undefined);
      }
    }
  }

  /** Read on the poll interval and publish, until the link breaks. */
  private async pollUntilDisconnected(
    cfg: DeviceConfig,
    session: DeviceSession,
    data: DataFacade,
    adapter: string,
    mailbox: Mailbox<WriteRequest>,
    health: Health,
  ): Promise<void> {
    let lastHealth = Date.now();

    while (!this.stopped) {
      // Writes and polls share this one loop, so a write can never race a read on the same
      // connection — most device protocols are a single request/response channel and would
      // interleave into nonsense if two callers talked at once.
      const deadline = Date.now() + cfg.pollIntervalMs;
      for (;;) {
        const remaining = deadline - Date.now();
        if (remaining <= 0) break;
        const req = await mailbox.receive(remaining);
        if (!req) break;
        try {
          await session.writeSignal(req.signalId, req.value);
          req.settle();
        } catch (e) {
          logger.warn(`write failed instance=${cfg.id} signal=${req.signalId}: ${String(e)}`);
          // The command handler is waiting on this: a write is confirmed, not assumed.
          req.settle(String(e));
        }
      }
      if (this.stopped) break;

      const started = Date.now();
      let readings: Reading[];
      try {
        readings = await session.readSignals();
      } catch (e) {
        // The link is gone. Leave the poll loop so the connect loop can back off.
        health.readErrors += 1;
        logger.warn(`read failed instance=${cfg.id}; reconnecting: ${String(e)}`);
        await session.close().catch(() => undefined);
        return;
      }
      health.pollLatencyMs = Date.now() - started;

      await publishReadings(data, adapter, cfg, readings, health);

      if (Date.now() - lastHealth >= HEALTH_INTERVAL_MS) {
        await this.emitHealth(health);
        lastHealth = Date.now();
      }
    }

    await session.close().catch(() => undefined);
  }

  private async emitHealth(health: Health): Promise<void> {
    await this.metrics
      .emitMetric(HEALTH_METRIC, health.takeInterval())
      .catch((e: unknown) => logger.warn(`health metric emit failed: ${String(e)}`));
  }

  /** Stop the device loops and clean up before the runtime is closed. */
  async stop(): Promise<void> {
    this.stopped = true;
    await Promise.allSettled(this.loops);
    await this.metrics.flushMetrics().catch(() => undefined);
  }
}
