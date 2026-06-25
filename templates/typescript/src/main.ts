/**
 * <<COMPONENTNAME>> — entry point.
 *
 * An AWS IoT Greengrass v2 component built on the `ggcommons` TypeScript library.
 * Initializes the runtime from the standard CLI contract (`-c`/`--platform`/`--transport`/`-t`),
 * then hands control to {@link App}. The component runs until a shutdown signal
 * (SIGINT / SIGTERM); it then awaits `gg.close()` to release all resources
 * (TypeScript has no RAII).
 *
 * ## Running locally (HOST platform, MQTT transport, against a local MQTT broker)
 * ```bash
 * node dist/main.js \
 *   --platform HOST --transport MQTT ./test-configs/standalone-messaging.json \
 *   -c FILE ./test-configs/config.json \
 *   -t my-thing
 * ```
 */
import { GGCommonsBuilder, logger } from "@breissinger/ggcommons";

import { App } from "./app";

/** The component's full name (matches `recipe.yaml` / `gdk-config.json`). */
const COMPONENT_NAME = "<<COMPONENTFULLNAME>>";

async function main(): Promise<void> {
  // `process.argv.slice(2)` drops the `node` and script-path prefix.
  const gg = await new GGCommonsBuilder(COMPONENT_NAME).args(process.argv.slice(2)).build();

  logger.info(`<<COMPONENTNAME>> starting: component=${gg.componentName()} thing=${gg.config().thingName}`);

  const app = new App(gg);

  // Graceful shutdown: stop the app, then release the runtime. SIGTERM is what
  // Greengrass sends to stop a component; SIGINT is Ctrl-C for local runs.
  const shutdown = async (signal: string): Promise<void> => {
    logger.info(`${signal} received; shutting down`);
    await app.stop();
    await gg.close();
    logger.info("<<COMPONENTNAME>> stopped");
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
