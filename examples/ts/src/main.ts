/**
 * TypeScript Component Skeleton — entry point.
 *
 * A worked-example AWS IoT Greengrass v2 component built on the `ggcommons`
 * TypeScript library, mirroring the Java, Python, and Rust skeletons. It
 * initializes the runtime from the standard CLI contract (`-c`/`-m`/`-t`), then
 * hands control to {@link SkeletonApp}, which demonstrates the library's
 * messaging, configuration, metrics, and heartbeat features. The component runs
 * until it receives a shutdown signal (SIGINT / SIGTERM), at which point it
 * awaits `gg.close()` so all resources are released (TypeScript has no RAII).
 *
 * ## Running locally (STANDALONE mode, against a local MQTT broker)
 * ```bash
 * node dist/main.js \
 *   -m STANDALONE ./test-configs/standalone-messaging.json \
 *   -c FILE ./test-configs/config.json \
 *   -t my-thing
 * ```
 */
import { GGCommonsBuilder, logger } from "ggcommons";

import { SkeletonApp } from "./app";

/** The component's full name (matches `recipe.yaml` / `gdk-config.json`). */
const COMPONENT_NAME = "aws.proserve.greengrass.TsComponentSkeleton";

/** Boot the component: build the runtime from CLI args, run the app, shut down cleanly. */
async function main(): Promise<void> {
  // `process.argv.slice(2)` drops the `node` and script-path prefix.
  const gg = await new GGCommonsBuilder(COMPONENT_NAME).args(process.argv.slice(2)).build();

  logger.info(
    `TypeScript Component Skeleton starting: component=${gg.componentName()} thing=${gg.config().thingName}`,
  );

  const app = new SkeletonApp(gg);

  // Graceful shutdown: stop the app loops, then release the runtime (the Node
  // counterpart of dropping the Rust runtime / RAII). SIGTERM is what Greengrass
  // sends to stop a component; SIGINT is Ctrl-C for local runs.
  const shutdown = async (signal: string): Promise<void> => {
    logger.info(`${signal} received; shutting down`);
    await app.stop();
    await gg.close();
    logger.info("TypeScript Component Skeleton stopped");
    process.exit(0);
  };
  process.on("SIGINT", () => void shutdown("SIGINT"));
  process.on("SIGTERM", () => void shutdown("SIGTERM"));

  await app.run();
}

main().catch((err) => {
  // eslint-disable-next-line no-console
  console.error("fatal:", err);
  process.exit(1);
});
