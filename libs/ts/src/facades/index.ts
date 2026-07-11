/**
 * The app-usable class publish facades (DESIGN-class-facades): `data()`/`events()`/`app()` —
 * body-contract enforcement + defaults for the `data`/`evt`/`app` UNS classes, the non-reserved
 * siblings of the reserved publishers (`state`/`metric`/`cfg`). Obtain bound instances from
 * `EdgeCommons.data()`/`.events()`/`.app()` (the `main`-instance convenience) or
 * `EdgeCommonsInstance.data()`/`.events()`/`.app()` (per-instance, primary) — see `edgecommons.ts`.
 */
export { DataFacade, DATA_MESSAGE_NAME, DATA_MESSAGE_VERSION, QUALITY_UNSPECIFIED } from "./data_facade";
export { EventsFacade, EVT_MESSAGE_NAME, EVT_MESSAGE_VERSION } from "./events_facade";
export { AppFacade, PreparedAppMessage, APP_MESSAGE_VERSION } from "./app_facade";

export { Quality, qualityFromWire } from "./quality";
export { Severity, severityFromWire } from "./severity";
export { Channel } from "./channel";
export type { ChannelKind, LocalChannel, NorthboundChannel, StreamChannel } from "./channel";

export { SignalUpdateBuilder, effectiveSignalPath } from "./signal_update";
export type { Sample, SampleOptions, SignalUpdate } from "./signal_update";

export type { StreamSink } from "./stream_sink";
export type { ClockMillis } from "./clock";
export { toIso } from "./clock";
