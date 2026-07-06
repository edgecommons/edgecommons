/**
 * Telemetry streaming for Node/TypeScript components — durable store-and-forward over the shared
 * Rust `edgestreamlog` core (napi-rs native addon). See {@link StreamService}.
 */
export { StreamService, StreamHandle, EdgeStreamError } from "./service";
export type { StreamStats } from "./service";
export { StreamMetricsBridge } from "./bridge";
