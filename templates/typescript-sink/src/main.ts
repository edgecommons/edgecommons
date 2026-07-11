/**
 * <<COMPONENTNAME>> — entry point.
 *
 * An AWS IoT Greengrass v2 component built on the `edgecommons` TypeScript library.
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
import { EdgeCommonsBuilder, logger } from "@edgecommons/edgecommons";

import { App } from "./app";

/** The component's full name (matches `recipe.yaml` / `gdk-config.json`). */
const COMPONENT_NAME = "<<COMPONENTFULLNAME>>";

async function main(): Promise<void> {
  // `process.argv.slice(2)` drops the `node` and script-path prefix.
  const gg = await new EdgeCommonsBuilder(COMPONENT_NAME).args(process.argv.slice(2)).build();

  logger.info(`<<COMPONENTNAME>> starting: component=${gg.componentName()} thing=${gg.config().thingName}`);

  const app = new App(gg);

  // The edgecommons runtime owns SIGTERM/SIGINT (FR-HB-2): on a termination signal it flips
  // `/readyz` to 503, runs the idempotent graceful close, removes its handlers, and exits the
  // process. Do NOT register your own `process.on("SIGTERM"/"SIGINT")` handler or call
  // `process.exit()` — that double-runs teardown and can cut off the library's async close. The
  // active runtime (messaging, heartbeat, health server) keeps the process alive until that signal.
  // The try/finally below only covers a normal (non-signal) return from `run()`.
  try {
    await app.run();
  } finally {
    await app.stop();
    await gg.close();
    logger.info("<<COMPONENTNAME>> stopped");
  }
}

main().catch((err) => {
  // eslint-disable-next-line no-console
  console.error("fatal:", err);
  process.exit(1);
});
