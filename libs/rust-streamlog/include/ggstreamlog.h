/*
 * ggstreamlog.h — C ABI for the ggcommons telemetry-streaming core.
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
 *   - `ggsl_service*` is an opaque handle owned by the core; free it with ggsl_shutdown.
 *   - `ggsl_stream*` is a caller-owned handle (an internal ref-count to the stream); free it with
 *     ggsl_stream_free. It remains valid for append/flush even after ggsl_shutdown (export stops,
 *     but the durable buffer stays usable), so handle/service teardown order does not matter.
 *   - On error, `*err` is set to a heap-allocated, NUL-terminated UTF-8 string the caller MUST
 *     release with ggsl_str_free. On success `*err` is left unchanged (set it to NULL first).
 *   - `ggsl_stats` writes into a caller-provided struct (no allocation).
 *
 * ERRORS & PANIC SAFETY
 *   - Every fallible function returns 0 on success and a non-zero ggsl_status on failure.
 *   - EVERY entry point wraps the Rust core in catch_unwind: a panic never crosses the FFI
 *     boundary — it is converted into GGSL_ERR_PANIC with a message in *err.
 *
 * THREAD SAFETY
 *   - ggsl_append / ggsl_flush / ggsl_stats are thread-safe and may be called concurrently from
 *     many host threads on the same stream (the core serializes internally).
 *   - ggsl_open / ggsl_shutdown are lifecycle calls; do not call ggsl_shutdown concurrently with
 *     other calls on the same service.
 */

#ifndef GGSTREAMLOG_H
#define GGSTREAMLOG_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ---- Opaque handles ----------------------------------------------------- */

typedef struct ggsl_service ggsl_service; /* one process-wide set of streams */
typedef struct ggsl_stream  ggsl_stream;  /* one durable stream + its export engine */

/* ---- Status codes ------------------------------------------------------- */

typedef enum ggsl_status {
    GGSL_OK            = 0, /* success */
    GGSL_ERR_CONFIG    = 1, /* invalid configuration / JSON */
    GGSL_ERR_IO        = 2, /* filesystem / durability error */
    GGSL_ERR_CORRUPT   = 3, /* on-disk corruption detected */
    GGSL_ERR_FULL      = 4, /* buffer full under onFull=rejectNew */
    GGSL_ERR_UNKNOWN_STREAM = 5, /* no stream with the given name */
    GGSL_ERR_SINK      = 6, /* sink / export error */
    GGSL_ERR_PANIC     = 7, /* a Rust panic was caught at the boundary */
    GGSL_ERR_INVALID_ARG = 8 /* NULL handle / bad pointer / bad length */
} ggsl_status;

/* ---- Stats snapshot (caller-provided, numeric only) --------------------- */

/*
 * Mirrors the Rust `Stats` (docs §10). Numeric-only so the struct is a stable POD across the
 * ABI; the optional last-export-error string is retrieved separately in Phase 2 (it is not a
 * fixed-width field). All counters are cumulative since the stream was opened, except `backlog`,
 * `disk_bytes`, `*_offset`, and `oldest_unacked_age_ms`, which are instantaneous gauges.
 */
typedef struct ggsl_stats_t {
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
} ggsl_stats_t;

/* ---- Lifecycle ---------------------------------------------------------- */

/*
 * Open + recover every stream described by `config_json` (the `streaming` config section:
 * { "streams": [ { "name", "sink", "buffer", "batch", "delivery" }, ... ] }). On success
 * `*out` receives a service handle. Background export engines start immediately.
 */
int ggsl_open(const char* config_json, ggsl_service** out, char** err);

/*
 * Look up a configured stream by name. `*out` receives a caller-owned handle; release it with
 * ggsl_stream_free (it stays valid for append/flush even after ggsl_shutdown).
 */
int ggsl_stream_get(ggsl_service* service, const char* name, ggsl_stream** out, char** err);

/* Release a stream handle obtained from ggsl_stream_get. NULL is a no-op. */
void ggsl_stream_free(ggsl_stream* stream);

/* Flush in-memory buffers durably to disk + stop all export engines + free the service. */
void ggsl_shutdown(ggsl_service* service);

/* ---- Hot path ----------------------------------------------------------- */

/*
 * Append one record. `pk`/`payload` are borrowed for the call. The assigned offset is written to
 * `*out_offset`. Behavior when the disk budget is exceeded follows the stream's `onFull` policy
 * (dropOldest never blocks; block waits for export to reclaim space; rejectNew returns
 * GGSL_ERR_FULL). Thread-safe.
 */
int ggsl_append(ggsl_stream* stream,
                const uint8_t* pk, uint16_t pk_len,
                uint64_t ts_ms,
                const uint8_t* payload, uint32_t payload_len,
                uint64_t* out_offset, char** err);

/* Force this stream's buffer durably to disk. Does NOT wait for export to the sink. */
int ggsl_flush(ggsl_stream* stream, char** err);

/*
 * Write a stats snapshot for the named stream into the caller-provided struct. Takes the service
 * (not a stream handle) because export counters live with the engine the service owns. Returns
 * GGSL_ERR_UNKNOWN_STREAM if `name` is not configured, GGSL_ERR_INVALID_ARG on NULL.
 */
int ggsl_stats(ggsl_service* service, const char* name, ggsl_stats_t* out);

/* ---- Memory ------------------------------------------------------------- */

/* Free a heap string returned via an `err` out-parameter. NULL is a no-op. */
void ggsl_str_free(char* s);

/* ---- Phase-3: pluggable credential callback (Kafka SASL/OAuth, mTLS) ----- */

/*
 * Reserved for Phase 3. A host-supplied callback the core invokes to obtain fresh credentials
 * for sinks that need them outside the AWS SDK chain (e.g. Kafka). The callback must be
 * thread-safe and must not block indefinitely. `user_data` is passed through verbatim.
 *
 * Not used in Phase 1/2 (Kinesis uses the AWS default credential chain, which already covers the
 * Greengrass TES container-credentials endpoint with no host involvement).
 */
typedef int (*ggsl_credential_cb)(void* user_data, char** out_credentials_json, char** err);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* GGSTREAMLOG_H */
