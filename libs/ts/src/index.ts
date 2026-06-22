/**
 * GGCommons TypeScript library — public surface.
 *
 * A 4th implementation of the Greengrass Commons library alongside Java
 * (canonical), Python, and Rust. Bundles the cross-cutting concerns of an AWS IoT
 * Greengrass v2 component (configuration, messaging, metrics, heartbeat, logging)
 * behind service interfaces so component authors write only business logic.
 *
 * Typical entry point:
 * ```ts
 * import { GGCommonsBuilder } from "ggcommons";
 * const gg = await new GGCommonsBuilder("com.example.MyComponent").build();
 * const cfg = gg.config();
 * ```
 */

// Lifecycle
export { GGCommons, GGCommonsBuilder } from "./ggcommons";

// Errors
export { GgError } from "./errors";
export type { GgErrorKind } from "./errors";

// CLI
export { parseArgs } from "./cli";
export type { ParsedArgs, RuntimeMode, ConfigSourceSpec } from "./cli";

// Config
export {
  Config,
  LoggingConfig,
  FileLoggingConfig,
  HeartbeatConfig,
  MetricConfig,
  resolve,
  validate,
  buildConfigSource,
} from "./config";
export type {
  RawConfig,
  Measures,
  HeartbeatTarget,
  ComponentConfig,
  ConfigurationChangeListener,
  ConfigSource,
} from "./config";

// Messaging
export {
  Message,
  MessageBuilder,
  Destination,
  Qos,
  ReplyFuture,
  REPLY_TOPIC_PREFIX,
  DefaultMessagingService,
  StandaloneMqttProvider,
  IpcMessagingProvider,
  topicMatches,
  loadMessagingConfig,
  resolvedHost,
} from "./messaging";
export type {
  MessageHeader,
  MessageTags,
  MessageHandler,
  MessagingProvider,
  IMessagingService,
  RawSubscription,
  IpcOptions,
  MessagingConfig,
  BrokerConfig,
  Credentials,
} from "./messaging";

// Metrics
export {
  Metric,
  Measure,
  MetricBuilder,
  MetricEmitter,
  buildEmf,
  buildEmfVariants,
} from "./metrics";
export type { MetricService, MetricTarget, MeasureValues } from "./metrics";

// Heartbeat
export { Heartbeat, HeartbeatMonitor } from "./heartbeat";
export type { ConfigProvider } from "./heartbeat";

// Logging
export { logger, initLogging, reconfigureLogging, LoggingReconfigurer, Logger } from "./logging";

// Telemetry streaming
export { StreamService, StreamHandle, GgStreamError, StreamMetricsBridge } from "./streaming";
export type { StreamStats } from "./streaming";

// Credentials / local vault
export {
  openFromConfig as openCredentials,
  CredentialError,
  DefaultCredentialService,
  FileKeyProvider,
  LocalVault,
  Secret,
  LogAuditSink,
  logSink,
} from "./credentials";
export type { CredentialService, CredentialsConfig, KeyProvider, PutOptions, SecretMeta, AuditEvent, AuditSink } from "./credentials";

// Parameters (gg.parameters())
export {
  openFromConfig as openParameters,
  ParameterError,
  DefaultParameterService,
  EnvSource,
  MountedDirSource,
  AwsSsmSource,
} from "./parameters";
export type {
  ParameterService,
  ParametersConfig,
  ParameterStats,
  ParamValue,
  ParameterSource,
  PathEntry,
} from "./parameters";
