/**
 * TypeScript Component Skeleton — entry point.
 *
 * A worked-example AWS IoT Greengrass v2 component built on the `ggcommons`
 * TypeScript library, mirroring the Java, Python, and Rust skeletons. It
 * initializes the runtime from the standard CLI contract
 * (`-c`/`--platform`/`--transport`/`-t`), then
 * hands control to {@link SkeletonApp}, which demonstrates the library's
 * messaging, configuration, metrics, and heartbeat features. The component runs
 * until it receives a shutdown signal (SIGINT / SIGTERM): the **library** wires
 * those signals (Phase 1c / FR-HB-2) — flipping `/readyz` to 503 and then awaiting
 * `gg.close()` so all resources are released (TypeScript has no RAII) before
 * exiting 0 — so the skeleton no longer installs its own handlers.
 *
 * ## Running locally (HOST platform, MQTT transport, against a local MQTT broker)
 * ```bash
 * node dist/main.js \
 *   --platform HOST --transport MQTT ./test-configs/standalone-messaging.json \
 *   -c FILE ./test-configs/config.json \
 *   -t my-thing
 * ```
 */
import { GGCommonsBuilder, logger } from "@mbreissi/ggcommons";

import { SkeletonApp } from "./app";

/** The component's full name (matches `recipe.yaml` / `gdk-config.json`). */
const COMPONENT_NAME = "com.mbreissi.greengrass.TsComponentSkeleton";

/** Boot the component: build the runtime from CLI args, run the app, shut down cleanly. */
async function main(): Promise<void> {
  // `process.argv.slice(2)` drops the `node` and script-path prefix.
  const gg = await new GGCommonsBuilder(COMPONENT_NAME).args(process.argv.slice(2)).build();

  logger.info(
    `TypeScript Component Skeleton starting: component=${gg.componentName()} thing=${gg.config().thingName}`,
  );

  const app = new SkeletonApp(gg);

  // Graceful shutdown is owned by the LIBRARY now (Phase 1c / FR-HB-2): GGCommonsBuilder.build()
  // wires SIGTERM and SIGINT so that on receipt the runtime flips /readyz to 503, runs the
  // idempotent gg.close() (unsubscribe every tracked subscription + bounded-close
  // messaging/streams/heartbeat/vault), then exits 0. The skeleton therefore no longer installs its
  // own signal handlers — doing so would double-wire shutdown. SIGTERM is what Greengrass / the
  // kubelet send to stop a component; SIGINT is Ctrl-C for local runs.
  //
  // The component keeps running (the publish loop's timer + the messaging sockets hold the event
  // loop open) until the library's handler exits the process. A component that manages its own
  // background loops can still call `app.stop()` from its own logic to halt them early.
  await app.run();
}

main().catch((err) => {
  // eslint-disable-next-line no-console
  console.error("fatal:", err);
  process.exit(1);
});
