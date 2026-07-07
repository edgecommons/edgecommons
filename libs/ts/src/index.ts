/**
 * EdgeCommons TypeScript library — public surface.
 *
 * A 4th implementation of the Greengrass Commons library alongside Java
 * (canonical), Python, and Rust. Bundles the cross-cutting concerns of an AWS IoT
 * Greengrass v2 component (configuration, messaging, metrics, heartbeat, logging)
 * behind service interfaces so component authors write only business logic.
 *
 * Typical entry point:
 * ```ts
 * import { EdgeCommonsBuilder } from "@edgecommons/edgecommons";
 * const gg = await new EdgeCommonsBuilder("com.example.MyComponent").build();
 * const cfg = gg.config();
 * ```
 */

// Lifecycle
export { EdgeCommons, EdgeCommonsBuilder, EdgeCommonsInstance } from "./edgecommons";

// Unified namespace (UNS): topic builder/validator + reserved-class predicate
export {
  Uns,
  UnsClass,
  UnsScope,
  UnsValidationError,
  UNS_ROOT,
  MAX_TOPIC_SLASHES,
  MAX_TOPIC_UTF8_BYTES,
  RESERVED_CLASSES,
  isLeafClass,
  unsClassFromToken,
  checkToken,
  reservedClassOf,
} from "./uns";
export type { UnsValidationCode } from "./uns";

// Errors
export { EdgeCommonsError } from "./errors";
export type { EdgeCommonsErrorKind } from "./errors";

// CLI
export { parseArgs } from "./cli";
export type { ParsedArgs, ConfigSourceSpec } from "./cli";

// Platform × transport runtime model
export {
  Platform,
  Transport,
  PROFILES,
  JSON_LOG_FORMAT,
  resolveProfile,
  detectPlatform,
  profileLoggingFormat,
  profileHealthEnabled,
  validate as validatePlatformTransport,
  resolveIdentity,
} from "./platform";
export type { PlatformProfile, ResolvedProfile, ResolverInputs } from "./platform";

// Config
export {
  Config,
  LoggingConfig,
  FileLoggingConfig,
  HeartbeatConfig,
  MetricConfig,
  HealthConfig,
  DEFAULT_REQUEST_TIMEOUT_SECONDS,
  resolve,
  sanitize,
  validate,
  buildConfigSource,
} from "./config";
export type {
  RawConfig,
  Measures,
  ComponentConfig,
  ConfigurationChangeListener,
  ConfigSource,
} from "./config";
export { EffectiveConfigPublisher, redact } from "./config/effective_config";

// Messaging
export {
  Message,
  MessageBuilder,
  MessageBodyCase,
  MessageIdentity,
  Destination,
  Qos,
  ReplyFuture,
  RequestTimeoutError,
  ReservedTopicError,
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
  HierLevel,
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
export { InstanceConnectivity } from "./instance_connectivity";
export type { InstanceConnectivityProvider } from "./instance_connectivity";

// UNS `_bcast` republish listener (the late-join lever, DESIGN-uns §9.3/§9.4)
export { RepublishListener } from "./republish_listener";
export type { Delayer as RepublishDelayer, ClockMillis as RepublishClockMillis, JitterFn as RepublishJitterFn } from "./republish_listener";

// Command inbox — the minimal `commands()` facade (DESIGN-uns §7.3/§9.5, edge-console slice S2)
export { CommandInbox, CommandException } from "./commands";
export type { CommandHandler, CommandResult } from "./commands";

// App-usable class publish facades: data()/events()/app() (DESIGN-class-facades)
export {
  DataFacade,
  DATA_MESSAGE_NAME,
  DATA_MESSAGE_VERSION,
  QUALITY_UNSPECIFIED,
  EventsFacade,
  EVT_MESSAGE_NAME,
  EVT_MESSAGE_VERSION,
  AppFacade,
  APP_MESSAGE_VERSION,
  Quality,
  qualityFromWire,
  Severity,
  severityFromWire,
  Channel,
  SignalUpdateBuilder,
  effectiveSignalPath,
  toIso,
} from "./facades";
export type {
  ChannelKind,
  LocalChannel,
  NorthboundChannel,
  StreamChannel,
  Sample,
  SampleOptions,
  SignalUpdate,
  StreamSink,
  ClockMillis,
} from "./facades";

// Health (HTTP /livez · /readyz · /startupz + readiness state)
export { HealthServer, ReadinessState, evaluateHealth } from "./health";
export type { HealthServerOptions, HealthPaths, HealthResponse } from "./health";

// Logging
export { logger, getLogger, initLogging, reconfigureLogging, LoggingReconfigurer, Logger } from "./logging";
export type { LoggingOptions } from "./logging";

// Telemetry streaming
export { StreamService, StreamHandle, EdgeStreamError, StreamMetricsBridge } from "./streaming";
export type { StreamStats } from "./streaming";

// Credentials / local vault
export {
  openFromConfig as openCredentials,
  CredentialError,
  DefaultCredentialService,
  EnvKeyProvider,
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
