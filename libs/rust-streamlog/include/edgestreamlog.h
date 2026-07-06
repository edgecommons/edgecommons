/*
 * edgestreamlog.h — C ABI for the edgecommons telemetry-streaming core.
 *
 * STATUS: Phase 1 DESIGN ARTIFACT. This header finalizes the boundary so the Phase-2
 * language bindings (Panama/Java 21, PyO3/maturin/Python, napi-rs/Node) are not painted
 * into a corner. The Rust implementation (`src/ffi.rs`, behind the `cabi` cargo feature)
 * is written in Phase 2; until then no `cdylib`/`staticlib` exports these symbols.
 *
 * See docs/TELEMETRY_STREAMING_PHASE1.md §11. This SUPERSEDES the read_batch/commit sketch
 * in TELEMETRY_STREAMING.md §5.3: the export engine + sinks live entirely in the core, so the
 * host never drives export. The ABI is just append / flush / stats / lifecycle (+ a Phase-3
 * credential callback for Kafka). Configuration is passed as a JSON string (the same schema as
 * the `streaming` config section) to avoid a wide, version-fragile struct ABI.
 *
 * MEMORY & OWNERSHIP
 *   - Inputs (pointers/buffers) are BORROWED for the duration of the call; the caller retains
 *     ownership and may free them after the call returns.
 *   - `esl_service*` is an opaque handle owned by the core; free it with esl_shutdown.
 *   - `esl_stream*` is a caller-owned handle (an internal ref-count to the stream); free it with
 *     esl_stream_free. It remains valid for append/flush even after esl_shutdown (export stops,
 *     but the durable buffer stays usable), so handle/service teardown order does not matter.
 *   - On error, `*err` is set to a heap-allocated, NUL-terminated UTF-8 string the caller MUST
 *     release with esl_str_free. On success `*err` is left unchanged (set it to NULL first).
 *   - `esl_stats` writes into a caller-provided struct (no allocation).
 *
 * ERRORS & PANIC SAFETY
 *   - Every fallible function returns 0 on success and a non-zero esl_status on failure.
 *   - EVERY entry point wraps the Rust core in catch_unwind: a panic never crosses the FFI
 *     boundary — it is converted into ESL_ERR_PANIC with a message in *err.
 *
 * THREAD SAFETY
 *   - esl_append / esl_flush / esl_stats are thread-safe and may be called concurrently from
 *     many host threads on the same stream (the core serializes internally).
 *   - esl_open / esl_shutdown are lifecycle calls; do not call esl_shutdown concurrently with
 *     other calls on the same service.
 */

#ifndef EDGESTREAMLOG_H
#define EDGESTREAMLOG_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ---- Opaque handles ----------------------------------------------------- */

typedef struct esl_service esl_service; /* one process-wide set of streams */
typedef struct esl_stream  esl_stream;  /* one durable stream + its export engine */

/* ---- Status codes ------------------------------------------------------- */

typedef enum esl_status {
    ESL_OK            = 0, /* success */
    ESL_ERR_CONFIG    = 1, /* invalid configuration / JSON */
    ESL_ERR_IO        = 2, /* filesystem / durability error */
    ESL_ERR_CORRUPT   = 3, /* on-disk corruption detected */
    ESL_ERR_FULL      = 4, /* buffer full under onFull=rejectNew */
    ESL_ERR_UNKNOWN_STREAM = 5, /* no stream with the given name */
    ESL_ERR_SINK      = 6, /* sink / export error */
    ESL_ERR_PANIC     = 7, /* a Rust panic was caught at the boundary */
    ESL_ERR_INVALID_ARG = 8 /* NULL handle / bad pointer / bad length */
} esl_status;

/* ---- Stats snapshot (caller-provided, numeric only) --------------------- */

/*
 * Mirrors the Rust `Stats` (docs §10). Numeric-only so the struct is a stable POD across the
 * ABI; the optional last-export-error string is retrieved separately in Phase 2 (it is not a
 * fixed-width field). All counters are cumulative since the stream was opened, except `backlog`,
 * `disk_bytes`, `*_offset`, and `oldest_unacked_age_ms`, which are instantaneous gauges.
 */
typedef struct esl_stats_t {
    uint64_t appended_total;        /* records accepted into the buffer */
    uint64_t exported_total;        /* records acked by the sink */
    uint64_t dropped_total;         /* records dropped by retention (onFull=dropOldest) */
    uint64_t retries_total;         /* sink send retries */
    uint64_t failed_total;          /* records abandoned after max_retries (poison-pill) */
    uint64_t backlog;               /* un-exported records currently buffered */
    uint64_t disk_bytes;            /* on-disk footprint of this stream */
    uint64_t acked_offset;          /* export cursor (next offset to export) */
    uint64_t next_offset;           /* next offset to be assigned on append */
    uint64_t oldest_unacked_age_ms; /* age of the oldest un-exported record (0 if empty) */
} esl_stats_t;

/* ---- Lifecycle ---------------------------------------------------------- */

/*
 * Open + recover every stream described by `config_json` (the `streaming` config section:
 * { "streams": [ { "name", "sink", "buffer", "batch", "delivery" }, ... ] }). On success
 * `*out` receives a service handle. Background export engines start immediately.
 */
int esl_open(const char* config_json, esl_service** out, char** err);

/*
 * Look up a configured stream by name. `*out` receives a caller-owned handle; release it with
 * esl_stream_free (it stays valid for append/flush even after esl_shutdown).
 */
int esl_stream_get(esl_service* service, const char* name, esl_stream** out, char** err);

/* Release a stream handle obtained from esl_stream_get. NULL is a no-op. */
void esl_stream_free(esl_stream* stream);

/* Flush in-memory buffers durably to disk + stop all export engines + free the service. */
void esl_shutdown(esl_service* service);

/* ---- Hot path ----------------------------------------------------------- */

/*
 * Append one record. `pk`/`payload` are borrowed for the call. The assigned offset is written to
 * `*out_offset`. Behavior when the disk budget is exceeded follows the stream's `onFull` policy
 * (dropOldest never blocks; block waits for export to reclaim space; rejectNew returns
 * ESL_ERR_FULL). Thread-safe.
 */
int esl_append(esl_stream* stream,
                const uint8_t* pk, uint16_t pk_len,
                uint64_t ts_ms,
                const uint8_t* payload, uint32_t payload_len,
                uint64_t* out_offset, char** err);

/* Force this stream's buffer durably to disk. Does NOT wait for export to the sink. */
int esl_flush(esl_stream* stream, char** err);

/*
 * Write a stats snapshot for the named stream into the caller-provided struct. Takes the service
 * (not a stream handle) because export counters live with the engine the service owns. Returns
 * ESL_ERR_UNKNOWN_STREAM if `name` is not configured, ESL_ERR_INVALID_ARG on NULL.
 */
int esl_stats(esl_service* service, const char* name, esl_stats_t* out);

/* ---- Memory ------------------------------------------------------------- */

/* Free a heap string returned via an `err` out-parameter. NULL is a no-op. */
void esl_str_free(char* s);

/* ---- Log forwarding -------------------------------------------------------- */

/*
 * Host log callback: receives the core's log events so the host logger (log4j2 / Python logging /
 * Node) can emit them. `level` is 1=ERROR, 2=WARN, 3=INFO, 4=DEBUG, 5=TRACE. `target` is the source
 * module; `message` the formatted text. Both strings are valid ONLY for the duration of the call
 * (copy them if retained). The callback may be invoked from background threads (export/maintenance),
 * so it must be thread-safe and must NOT call back into edgestreamlog. `user_data` is passed verbatim.
 */
typedef void (*esl_log_cb)(void* user_data, int level, const char* target, const char* message);

/*
 * Register (or clear, with cb = NULL) the host log callback. Idempotent; the forwarding subscriber
 * is installed on first registration. The host applies its own level filtering. Returns ESL_OK.
 */
int esl_set_log_callback(esl_log_cb cb, void* user_data);

/* ---- Host sink callback (CloudWatch metrics drain / bring-your-own-sink) ----- */

/*
 * Outcome status the host writes into esl_sink_outcome_t.status. DISTINCT from esl_status above:
 * these describe a sink send, not an API call.
 */
typedef enum esl_sink_status {
    ESL_SINK_ALL_ACKED        = 0, /* every record stored; advance the checkpoint past the batch */
    ESL_SINK_PARTIAL          = 1, /* only failed_offsets[0..failed_count] failed; retry just those */
    ESL_SINK_FAILED_RETRYABLE = 2, /* whole batch failed, may succeed later (disconnect/throttle/5xx) */
    ESL_SINK_FAILED_PERMANENT = 3  /* whole batch failed permanently (host should have dropped it) */
} esl_sink_status;

/*
 * One record handed to the host sink callback. All pointers BORROW the export batch and are valid
 * ONLY for the duration of the call (copy anything you retain). `pk`/`payload` may be NULL iff the
 * matching length is 0. For the CloudWatch drain, `pk` is the namespace and `payload` is the compact
 * {namespace, datum} JSON.
 */
typedef struct esl_sink_record_t {
    uint64_t       offset;       /* log offset (use it to populate failed_offsets for a partial) */
    uint64_t       ts_ms;        /* record timestamp (epoch millis) */
    const uint8_t* pk;           /* partition key bytes (UTF-8) */
    size_t         pk_len;
    const uint8_t* payload;      /* record payload bytes */
    size_t         payload_len;
} esl_sink_record_t;

/*
 * The host fills this to report a batch's outcome. The core supplies `failed_offsets` pre-allocated
 * with room for `failed_cap` entries (== the batch length). For ESL_SINK_PARTIAL the host writes the
 * offsets that were NOT stored into that buffer and sets `failed_count` (<= failed_cap); for any other
 * status `failed_count` is ignored. `status` defaults to ESL_SINK_FAILED_RETRYABLE, so a batch the
 * host leaves untouched is retried, never silently acked.
 */
typedef struct esl_sink_outcome_t {
    int       status;            /* a esl_sink_status value */
    uint64_t* failed_offsets;    /* core-owned, host-written-into (capacity = failed_cap) */
    size_t    failed_cap;        /* number of slots in failed_offsets (== batch length) */
    size_t    failed_count;      /* host writes the number of failed offsets here (PARTIAL only) */
} esl_sink_outcome_t;

/*
 * Host sink callback: invoked on the export engine thread with a BORROWED batch of `n` records. The
 * host performs the send (e.g. CloudWatch PutMetricData) and writes the result into `*outcome`, then
 * returns ESL_OK. A non-zero return is treated as a retryable failure. The callback must be
 * thread-safe and return promptly — it blocks that stream's drain — and must NOT call back into
 * edgestreamlog. `user_data` is passed verbatim. A panic/exception that crosses the boundary is caught
 * and treated as a retryable failure (the batch is held, not lost).
 */
typedef int (*esl_sink_cb)(void* user_data,
                            const esl_sink_record_t* records, size_t n,
                            esl_sink_outcome_t* outcome);

/*
 * Register (or clear, with cb = NULL) the host sink callback used to drain streams whose sink is
 * { "type": "callback" }. Call this BEFORE esl_open: the binding is captured per stream at open
 * time, so a callback stream opened before registration is buffer-only (it persists but does not
 * export) until the service is reopened. `user_data` must remain valid until the callback is cleared
 * or the service is shut down. Returns ESL_OK.
 */
int esl_set_sink_callback(esl_sink_cb cb, void* user_data);

/* ---- Phase-3: pluggable credential callback (Kafka SASL/OAuth, mTLS) ----- */

/*
 * Reserved for Phase 3. A host-supplied callback the core invokes to obtain fresh credentials
 * for sinks that need them outside the AWS SDK chain (e.g. Kafka). The callback must be
 * thread-safe and must not block indefinitely. `user_data` is passed through verbatim.
 *
 * Not used in Phase 1/2 (Kinesis uses the AWS default credential chain, which already covers the
 * Greengrass TES container-credentials endpoint with no host involvement).
 */
typedef int (*esl_credential_cb)(void* user_data, char** out_credentials_json, char** err);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* EDGESTREAMLOG_H */
