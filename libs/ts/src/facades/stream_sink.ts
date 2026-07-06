/**
 * The seam {@link DataFacade} composes to route a `stream:<name>` channel into the telemetry
 * streaming service (DESIGN-class-facades §4: "the facade *composes* `StreamService`, it does
 * not replace it"). Production wires it to `streams().stream(name).append(partitionKey,
 * timestampMs, payload)` (see `EdgeCommons`'s `streamSink()` in `edgecommons.ts`); tests inject a
 * recording function so the facade never depends on the native `edgestreamlog` addon.
 *
 * When streaming is not configured (`streams() === undefined`), the instance handle passes
 * `undefined` and {@link DataFacade} falls the stream route back to a LOCAL publish
 * (readiness / no-streaming → local) rather than dropping the record.
 */

/**
 * Appends one durable record to a named stream.
 *
 * @param streamName   the configured stream name (the `stream:<name>` target)
 * @param partitionKey the routing/ordering key — the signal's stable `signal.id`
 * @param timestampMs  the producer timestamp (epoch millis, from the sample's `serverTs`)
 * @param payload      the serialized envelope bytes (the exact bytes a bus publish would carry)
 */
export type StreamSink = (streamName: string, partitionKey: string, timestampMs: number, payload: Buffer) => void;
