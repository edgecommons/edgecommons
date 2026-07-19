/**
 * # <<COMPONENTNAME>> — the runtime seam
 *
 * This is the **thin live-runtime seam**: it wires the `edgecommons` service handles together,
 * subscribes each route, and drives one loop per route. It needs a live runtime (a built
 * {@link EdgeCommons}, a messaging transport, a clock) to do anything, so it is validated by the
 * HOST / GREENGRASS / KUBERNETES deploy paths (the AGENTS.md validation matrix), not by a unit test
 * — and it is excluded from the coverage denominator in `vitest.config.ts`. Everything a test can
 * exercise without a live runtime — route parsing, the self-echo guard, the bounded queue, the
 * stats window, the identity restamp, the pipeline stages — lives in `src/app.ts` and `src/proc.ts`
 * and is covered there. Keep this file free of logic a test could exercise, so the exclusion stays
 * honest.
 */
import {
  Config,
  ConfigurationChangeListener,
  EdgeCommons,
  EventsFacade,
  IMessagingService,
  Message,
  MetricBuilder,
  MetricService,
  Qos,
  Severity,
  logger,
  resolve,
} from "@edgecommons/edgecommons";

import {
  BoundedQueue,
  METRIC_NAME,
  RouteConfig,
  Stats,
  instanceConnectivity,
  isSelfEcho,
  parseRoute,
  restamp,
} from "./app";
import { Pipeline, ProcMsg, buildStage } from "./proc";

const METRIC_INTERVAL_MS = 60_000;

const sleep = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

export class App {
  private readonly config: Config;
  private readonly metrics: MetricService;
  private readonly messaging?: IMessagingService;
  private readonly events?: EventsFacade;
  private readonly routes: RouteConfig[] = [];
  private readonly stats = new Stats();
  private readonly queues = new Map<string, BoundedQueue<ProcMsg>>();
  private readonly loops: Promise<void>[] = [];
  private stopped = false;

  constructor(gg: EdgeCommons) {
    this.config = gg.config();
    this.metrics = gg.metrics();

    const listener: ConfigurationChangeListener = {
      onConfigurationChange: (config: Config): boolean => {
        logger.info(`configuration changed (thing=${config.thingName})`);
        return true;
      },
    };
    gg.addConfigChangeListener(listener);

    // A processor with no transport is a processor with nothing to do — unlike the base scaffold,
    // this is fatal rather than a degrade-to-heartbeat.
    try {
      this.messaging = gg.messaging();
    } catch {
      throw new Error("a processor needs a messaging transport, and none was wired");
    }
    try {
      this.events = gg.events();
    } catch {
      this.events = undefined;
    }

    this.metrics.defineMetric(
      MetricBuilder.create(METRIC_NAME)
        .withConfig(this.config)
        .addMeasure("received", "Count", 60)
        .addMeasure("published", "Count", 60)
        .addMeasure("dropped", "Count", 60)
        .addMeasure("errors", "Count", 60)
        .build(),
    );

    // ONE provider, TWO surfaces: the library pushes this sample into the `state` keepalive's
    // `instances[]` every tick AND returns it from the built-in `status` verb when pulled, so a
    // console that subscribes and a console that asks can never disagree. See instanceConnectivity()
    // in src/app.ts for what a processor reports, and why.
    gg.setInstanceConnectivityProvider(instanceConnectivity);

    // One route per instance. A malformed route is skipped with a warning rather than killing the
    // component — but if EVERY route is malformed there is nothing to run, and failing loudly beats
    // idling silently.
    for (const id of this.config.instanceIds()) {
      try {
        const route = parseRoute(this.config.instance(id));
        // `publishTopic` goes through the library's config-template resolver, so a deployed route
        // can name `{ThingName}` / `{ComponentName}` / a hierarchy level / a tag and still address
        // the device it actually landed on. The literal template is preserved; only the substituted
        // values are sanitized.
        this.routes.push({ ...route, publishTopic: resolve(this.config, route.publishTopic) });
      } catch (e) {
        logger.warn(`skipping malformed route '${id}': ${String(e)}`);
      }
    }
    if (this.routes.length === 0) {
      throw new Error("no valid routes in component.instances[]");
    }
  }

  async run(): Promise<void> {
    const messaging = this.messaging;
    if (!messaging) throw new Error("no messaging transport");

    // Our own identity, captured once: the self-echo guard compares against it per message.
    const myPath = this.config.componentIdentity.path;
    const myComponent = this.config.componentIdentity.component;

    for (const route of this.routes) {
      const queue = new BoundedQueue<ProcMsg>(route.maxQueue);
      this.queues.set(route.id, queue);

      for (const filter of route.subscribe) {
        await messaging.subscribe(
          filter,
          (topic: string, msg: Message) => {
            if (isSelfEcho(msg, myPath, myComponent)) {
              return; // our own output; consuming it would loop forever
            }
            this.stats.received += 1;
            // A full queue must DROP and be COUNTED, never block the transport's dispatch.
            if (!queue.push({ topic, msg })) {
              this.stats.dropped += 1;
            }
          },
          route.maxQueue,
          1,
        );
        logger.info(`route=${route.id} subscribed filter=${filter}`);
      }

      this.loops.push(
        this.runRoute(route, queue, messaging).catch((e: unknown) =>
          logger.error(`route '${route.id}' stopped: ${String(e)}`),
        ),
      );
    }

    while (!this.stopped) {
      await sleep(METRIC_INTERVAL_MS);
      await this.emitMetrics();
    }
  }

  /**
   * One route's loop. Two arms, and they are the archetype: a message arrived → run the pipeline;
   * the tick fired → let stateful stages emit. A final tick on the way out emits a half-full window
   * rather than silently losing it.
   */
  private async runRoute(
    route: RouteConfig,
    queue: BoundedQueue<ProcMsg>,
    messaging: IMessagingService,
  ): Promise<void> {
    const pipeline = new Pipeline(route.pipeline.map(buildStage));

    while (!this.stopped) {
      const m = await queue.receive(route.tickMs);
      const out = m ? pipeline.run([m]) : pipeline.run([], Date.now());
      await this.dispatch(route, out, messaging);
    }

    // A final tick on the way out, so a half-full window is emitted rather than silently lost.
    await this.dispatch(route, pipeline.run([], Date.now()), messaging);
    logger.info(`route=${route.id} stopped`);
  }

  private async dispatch(route: RouteConfig, out: readonly ProcMsg[], messaging: IMessagingService): Promise<void> {
    for (const m of out) {
      // Restamp identity (src/app.ts): what we publish is OURS, not the producer's.
      const msg = restamp(this.config, m);

      try {
        if (route.target === "northbound") {
          await messaging.publishNorthbound(route.publishTopic, msg, Qos.AtLeastOnce);
        } else {
          await messaging.publish(route.publishTopic, msg);
        }
        this.stats.published += 1;
      } catch (e) {
        this.stats.errors += 1;
        logger.warn(`route=${route.id} publish failed: ${String(e)}`);
        await this.events
          ?.emit(Severity.Warning, "publish-failed", `route ${route.id} could not publish`, {
            route: route.id,
            topic: route.publishTopic,
          })
          .catch(() => undefined);
      }
    }
  }

  private async emitMetrics(): Promise<void> {
    await this.metrics
      .emitMetric(METRIC_NAME, this.stats.takeInterval())
      .catch((e: unknown) => logger.warn(`metric emit failed: ${String(e)}`));
  }

  /** Stop the route loops and clean up before the runtime is closed. */
  async stop(): Promise<void> {
    this.stopped = true;
    for (const queue of this.queues.values()) queue.close();
    await Promise.allSettled(this.loops);
    await this.emitMetrics();
    await this.metrics.flushMetrics().catch(() => undefined);
  }
}
