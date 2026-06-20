/*
 * C smoke test for the ggstreamlog C ABI (feature `cabi`). Validates the boundary the Phase-2
 * language bindings depend on: open from config JSON, get a stream, append, flush, stats, and the
 * unknown-stream error path, then free everything. Buffer-only (no `kinesis` feature) so it needs
 * no AWS. Build + run via ctest/run_smoke.sh.
 */
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#include "ggstreamlog.h"

#define N 1000

static int g_log_count = 0;

static void on_log(void *user_data, int level, const char *target, const char *message) {
    (void)user_data;
    g_log_count++;
    if (g_log_count <= 4) {
        fprintf(stderr, "[ggstreamlog L%d %s] %s\n", level, target, message);
    }
}

int main(void) {
    /* Register the log callback before open so the buffer-only warning is forwarded. */
    if (ggsl_set_log_callback(on_log, NULL) != GGSL_OK) {
        fprintf(stderr, "FAIL: ggsl_set_log_callback\n");
        return 1;
    }
    const char *cfg =
        "{\"streams\":[{"
        "\"name\":\"telemetry\","
        "\"sink\":{\"type\":\"kinesis\",\"streamName\":\"x\"},"
        "\"buffer\":{\"path\":\"/tmp/ggsl-smoke\",\"segmentBytes\":65536,"
        "\"maxDiskBytes\":1073741824,\"onFull\":\"block\"}"
        "}]}";

    ggsl_service *svc = NULL;
    char *err = NULL;

    int rc = ggsl_open(cfg, &svc, &err);
    if (rc != GGSL_OK) {
        fprintf(stderr, "ggsl_open failed: rc=%d err=%s\n", rc, err ? err : "(none)");
        ggsl_str_free(err);
        return 1;
    }

    ggsl_stream *s = NULL;
    rc = ggsl_stream_get(svc, "telemetry", &s, &err);
    if (rc != GGSL_OK) {
        fprintf(stderr, "ggsl_stream_get failed: rc=%d err=%s\n", rc, err ? err : "(none)");
        return 1;
    }

    const char *pk = "pump-7";
    for (int i = 0; i < N; i++) {
        char payload[32];
        int n = snprintf(payload, sizeof payload, "reading-%d", i);
        uint64_t off = 0;
        rc = ggsl_append(s, (const uint8_t *)pk, (uint16_t)strlen(pk), (uint64_t)(1000 + i),
                         (const uint8_t *)payload, (uint32_t)n, &off, &err);
        if (rc != GGSL_OK) {
            fprintf(stderr, "ggsl_append[%d] failed: rc=%d err=%s\n", i, rc, err ? err : "(none)");
            return 1;
        }
    }

    rc = ggsl_flush(s, &err);
    if (rc != GGSL_OK) {
        fprintf(stderr, "ggsl_flush failed: rc=%d err=%s\n", rc, err ? err : "(none)");
        return 1;
    }

    ggsl_stats_t st;
    memset(&st, 0, sizeof st);
    rc = ggsl_stats(svc, "telemetry", &st);
    if (rc != GGSL_OK) {
        fprintf(stderr, "ggsl_stats failed: rc=%d\n", rc);
        return 1;
    }
    printf("appended=%llu next_offset=%llu backlog=%llu disk_bytes=%llu dropped=%llu\n",
           (unsigned long long)st.appended_total, (unsigned long long)st.next_offset,
           (unsigned long long)st.backlog, (unsigned long long)st.disk_bytes,
           (unsigned long long)st.dropped_total);

    if (st.appended_total != N) {
        fprintf(stderr, "FAIL: appended_total=%llu, expected %d\n",
                (unsigned long long)st.appended_total, N);
        return 1;
    }
    if (st.next_offset != N) {
        fprintf(stderr, "FAIL: next_offset=%llu, expected %d\n",
                (unsigned long long)st.next_offset, N);
        return 1;
    }

    /* Unknown stream must report the dedicated error code, not crash. */
    rc = ggsl_stats(svc, "does-not-exist", &st);
    if (rc != GGSL_ERR_UNKNOWN_STREAM) {
        fprintf(stderr, "FAIL: unknown stream returned rc=%d, expected %d\n", rc,
                GGSL_ERR_UNKNOWN_STREAM);
        return 1;
    }

    /* NULL handling must be a no-op / clean error, never a crash. */
    ggsl_stream_free(NULL);
    ggsl_str_free(NULL);
    if (ggsl_flush(NULL, NULL) != GGSL_ERR_INVALID_ARG) {
        fprintf(stderr, "FAIL: ggsl_flush(NULL) did not return GGSL_ERR_INVALID_ARG\n");
        return 1;
    }

    ggsl_stream_free(s);
    ggsl_shutdown(svc);

    /* The core should have forwarded at least one log event (e.g. the buffer-only warning). */
    if (g_log_count == 0) {
        fprintf(stderr, "FAIL: no log events were forwarded to the callback\n");
        return 1;
    }

    printf("C smoke test PASSED (%d records appended, buffered, stats read back; %d log events)\n",
           N, g_log_count);
    return 0;
}
