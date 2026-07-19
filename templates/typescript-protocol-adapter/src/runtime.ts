/**
 * # <<COMPONENTNAME>> — the runtime seam
 *
 * This is the **thin live-runtime seam**: the `App` class wires the `edgecommons` service handles
 * together and runs one connect/poll/reconnect supervisor per device, servicing each device's
 * control channel as it goes. It needs a live runtime (a built {@link EdgeCommons}, a messaging
 * transport, real devices or the in-process simulator, a clock) to do anything, so it is validated
 * by the HOST / GREENGRASS / KUBERNETES deploy paths and by `test/live-sim.test.ts` on real infra —
 * not by a unit test — and it is excluded from the coverage denominator in `vitest.config.ts`.
 *
 * Everything a test can exercise without a live runtime lives in `src/app.ts` and is covered there:
 * config parsing ({@link parseDevice}/{@link buildDevices}), the reconnect backoff ({@link Backoff}),
 * the control mailbox ({@link Mailbox}), health/connectivity ({@link connectivityOf}), the southbound
 * publish path ({@link pollOnce}/{@link publishReadings}), and the per-message control decisions
 * ({@link handleControl}/{@link serveWhileDown}) this loop delegates to. Keep this file free of logic
 * a test could exercise, so the exclusion stays honest.
 */
import {
  Config,
  ConfigurationChangeListener,
  DataFacade,
  EdgeCommons,
  EventsFacade,
  MetricService,
  Severity,
  logger,
} from "@edgecommons/edgecommons";

import {
  Backoff,
  DeviceConfig,
  DeviceControl,
  Health,
  Mailbox,
  PollExit,
  ReconnectOutcome,
  buildDevices,
  connectivityOf,
  handleControl,
  pollOnce,
  readStaleSignalSecs,
  serveWhileDown,
} from "./app";
import { DeviceHandle, registerAll } from "./commands";
import { DeviceBackend, DeviceError, DeviceSession, SignalInfo, backendFor } from "./device";
import { DeviceMetrics } from "./metrics";

/** How often the periodic metrics emit runs, in the poll loop (ms). */
const METRICS_INTERVAL_MS = 30_000;

const rand01 = (): number => Math.random();

export class App {
  private readonly config: Config;
  private readonly metrics: MetricService;
  private readonly devices: DeviceConfig[];
  /** Each device's control channel: written by the command surface, drained by the device loop. */
  private readonly control = new Map<string, Mailbox<DeviceControl>>();
  /** Each device's health: written by its own loop, read by the connectivity provider. */
  private readonly health = new Map<string, Health>();
  /** Each device's operational-metrics emitter. */
  private readonly deviceMetrics = new Map<string, DeviceMetrics>();
  private readonly staleSignalSecs: number;
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

    this.staleSignalSecs = readStaleSignalSecs(this.config);
    // One device per instance (src/app.ts's buildDevices skips a malformed one, throws if none).
    this.devices = buildDevices(this.config);

    // A device's health exists from the moment it is CONFIGURED, not from the moment its loop first
    // connects: a configured device that is down must never be indistinguishable from a device
    // nobody configured. ONE provider, TWO surfaces: the library pushes this sample into the `state`
    // keepalive's `instances[]` every tick, and returns the very same sample from the built-in
    // `status` verb when a console asks. Whoever watches and whoever asks cannot get different answers.
    for (const device of this.devices) {
      this.health.set(device.id, new Health());
    }
    gg.setInstanceConnectivityProvider(() =>
      this.devices.map((d) => connectivityOf(d, this.health.get(d.id) as Health)),
    );
  }

  async run(): Promise<void> {
    // The per-device handles the command surface routes on.
    const handles: DeviceHandle[] = [];

    for (const device of this.devices) {
      // Per-instance facades: data() mints THIS device's topics and stamps its identity.
      const instance = this.gg.instance(device.id);

      const health = this.health.get(device.id) as Health;
      const dm = new DeviceMetrics(this.metrics, this.config, device.id, health, this.staleSignalSecs);
      // Pre-define the metric set so it is fixed and discoverable at startup.
      dm.defineAll();
      this.deviceMetrics.set(device.id, dm);

      // The signal inventory `sb/signals` shows — a config/backend view, no device round-trip.
      const backend = backendFor(device.adapter);
      const signals: SignalInfo[] = backend?.inventory?.(device.connection) ?? [];

      const control = new Mailbox<DeviceControl>();
      this.control.set(device.id, control);

      handles.push({ cfg: device, control, health, dm, signals });

      this.loops.push(
        this.runDevice(device, instance.data(), instance.events(), dm, health, control).catch(
          (e: unknown) => logger.error(`device loop '${device.id}' stopped: ${String(e)}`),
        ),
      );
    }

    // The southbound command surface (`src/commands.ts`). `ping` / `status` / `reload-config` /
    // `get-configuration` are already live — the library registered them before we ran.
    const commands = this.gg.commands();
    if (commands) {
      registerAll(commands, handles);
    }

    await Promise.all(this.loops);
  }

  /**
   * One device's lifecycle: connect, poll, publish, reconnect — and service its control channel.
   *
   * The connect loop and the poll loop are nested on purpose. A read failure that breaks the link
   * drops out of the poll loop and back into connect — the only place that knows how to back off.
   */
  private async runDevice(
    cfg: DeviceConfig,
    data: DataFacade,
    events: EventsFacade,
    dm: DeviceMetrics,
    health: Health,
    control: Mailbox<DeviceControl>,
  ): Promise<void> {
    const backend: DeviceBackend | undefined = backendFor(cfg.adapter);
    if (!backend) {
      logger.error(`unknown adapter '${cfg.adapter}' for instance '${cfg.id}'`);
      return;
    }

    const backoff = new Backoff();
    let attempt = 0;
    // A `reconnect` command's reply, held until the next connect settles it.
    let pendingReconnect: ((outcome: ReconnectOutcome) => void) | undefined;

    while (!this.stopped) {
      // --- CONNECT (servicing control while down, so pause/reconnect don't block on backoff) ---
      let session: DeviceSession | undefined;
      while (!this.stopped && session === undefined) {
        dm.onConnectAttempt();
        health.setLink(attempt === 0 ? "CONNECTING" : "BACKOFF");
        const now = Date.now();
        try {
          session = await backend.connect(cfg.connection);
          attempt = 0;
          dm.onConnected(now);
          health.setLink("ONLINE");
          await dm.emitNow();
          await events
            .emit(Severity.Info, "device-connected", `connected to ${cfg.connection.endpoint}`, {
              instance: cfg.id,
              adapter: backend.kind,
            })
            .catch(() => undefined);
          await events.clearAlarm("device-unreachable", { instance: cfg.id }).catch(() => undefined);
          if (pendingReconnect) {
            pendingReconnect({ ok: true });
            pendingReconnect = undefined;
          }
        } catch (e) {
          dm.onConnectFailure();
          if (pendingReconnect) {
            pendingReconnect({ ok: false, error: String(e) });
            pendingReconnect = undefined;
          }
          // A permanent failure fails identically forever — back off to the ceiling.
          const permanent = !DeviceError.isTransient(e);
          const wait = permanent ? backoff.maxMs : backoff.delayMs(attempt, rand01());
          attempt += 1;
          logger.warn(`connect failed instance=${cfg.id} permanent=${permanent} waitMs=${wait}: ${String(e)}`);
          const outcome = await serveWhileDown(control, events, health, wait);
          if (outcome.kind === "reconnect") {
            pendingReconnect = outcome.reply;
            attempt = 0;
          } else if (outcome.kind === "closed") {
            return;
          }
        }
      }
      if (session === undefined) return; // stopped while down

      // --- POLL (until the link breaks or a reconnect is requested) ---
      const exit = await this.runPolling(cfg, session, data, events, dm, health, control);

      // The link is down (or a reconnect asked us to drop it).
      health.setLink("BACKOFF");
      health.reconnects += 1;
      dm.onConnectionDropped(Date.now());
      await dm.emitNow();
      if (!this.stopped) {
        await events
          .raiseAlarm("device-unreachable", `lost the link to ${cfg.connection.endpoint}`, {
            instance: cfg.id,
          })
          .catch(() => undefined);
      }

      if (exit.kind === "closed") return;
      if (exit.kind === "reconnect") pendingReconnect = exit.reply;
      // linkLost: fall through and reconnect.
    }
  }

  /**
   * Read on the poll interval and publish, servicing the control channel, until the link breaks or
   * a reconnect is requested.
   */
  private async runPolling(
    cfg: DeviceConfig,
    session: DeviceSession,
    data: DataFacade,
    events: EventsFacade,
    dm: DeviceMetrics,
    health: Health,
    control: Mailbox<DeviceControl>,
  ): Promise<PollExit> {
    let nextPoll = Date.now() + cfg.pollIntervalMs;
    let nextMetrics = Date.now() + METRICS_INTERVAL_MS;

    while (!this.stopped) {
      const now = Date.now();
      const deadlines = [nextMetrics];
      if (!health.isPaused()) deadlines.push(nextPoll);
      const wait = Math.max(0, Math.min(...deadlines) - now);

      const ctrl = await control.receive(wait);
      if (control.isClosed() && ctrl === undefined) return { kind: "closed" };

      if (ctrl !== undefined) {
        // Poll and control share this one loop, so a write can never race a read on the same
        // connection — most device protocols are a single request/response channel.
        const exit = await handleControl(ctrl, cfg, session, data, events, dm, health);
        if (exit !== undefined) return exit;
      } else {
        if (!health.isPaused() && Date.now() >= nextPoll) {
          const r = await pollOnce(cfg, session, data, dm, health);
          if (!r.ok) {
            await session.close().catch(() => undefined);
            return { kind: "linkLost" };
          }
          nextPoll = Date.now() + cfg.pollIntervalMs;
        }
      }

      if (Date.now() >= nextMetrics) {
        await dm.emitPeriodic();
        nextMetrics = Date.now() + METRICS_INTERVAL_MS;
      }
    }
    await session.close().catch(() => undefined);
    return { kind: "closed" };
  }

  /** Stop the device loops and clean up before the runtime is closed. */
  async stop(): Promise<void> {
    this.stopped = true;
    for (const control of this.control.values()) control.close();
    await Promise.allSettled(this.loops);
    await this.metrics.flushMetrics().catch(() => undefined);
  }
}
