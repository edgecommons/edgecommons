/*
 * C smoke test for the edgestreamlog C ABI (feature `cabi`). Validates the boundary the Phase-2
 * language bindings depend on: open from config JSON, get a stream, append, flush, stats, and the
 * unknown-stream error path, then free everything. Buffer-only (no `kinesis` feature) so it needs
 * no AWS. Build + run via ctest/run_smoke.sh.
 */
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#include "edgestreamlog.h"

#define N 1000

static int g_log_count = 0;

static void on_log(void *user_data, int level, const char *target, const char *message) {
    (void)user_data;
    g_log_count++;
    if (g_log_count <= 4) {
        fprintf(stderr, "[edgestreamlog L%d %s] %s\n", level, target, message);
    }
}

int main(void) {
    /* Register the log callback before open so the buffer-only warning is forwarded. */
    if (esl_set_log_callback(on_log, NULL) != ESL_OK) {
        fprintf(stderr, "FAIL: esl_set_log_callback\n");
        return 1;
    }
    const char *cfg =
        "{\"streams\":[{"
        "\"name\":\"telemetry\","
        "\"sink\":{\"type\":\"kinesis\",\"streamName\":\"x\"},"
        "\"buffer\":{\"path\":\"/tmp/esl-smoke\",\"segmentBytes\":65536,"
        "\"maxDiskBytes\":1073741824,\"onFull\":\"block\"}"
        "}]}";

    esl_service *svc = NULL;
    char *err = NULL;

    int rc = esl_open(cfg, &svc, &err);
    if (rc != ESL_OK) {
        fprintf(stderr, "esl_open failed: rc=%d err=%s\n", rc, err ? err : "(none)");
        esl_str_free(err);
        return 1;
    }

    esl_stream *s = NULL;
    rc = esl_stream_get(svc, "telemetry", &s, &err);
    if (rc != ESL_OK) {
        fprintf(stderr, "esl_stream_get failed: rc=%d err=%s\n", rc, err ? err : "(none)");
        return 1;
    }

    const char *pk = "pump-7";
    for (int i = 0; i < N; i++) {
        char payload[32];
        int n = snprintf(payload, sizeof payload, "reading-%d", i);
        uint64_t off = 0;
        rc = esl_append(s, (const uint8_t *)pk, (uint16_t)strlen(pk), (uint64_t)(1000 + i),
                         (const uint8_t *)payload, (uint32_t)n, &off, &err);
        if (rc != ESL_OK) {
            fprintf(stderr, "esl_append[%d] failed: rc=%d err=%s\n", i, rc, err ? err : "(none)");
            return 1;
        }
    }

    rc = esl_flush(s, &err);
    if (rc != ESL_OK) {
        fprintf(stderr, "esl_flush failed: rc=%d err=%s\n", rc, err ? err : "(none)");
        return 1;
    }

    esl_stats_t st;
    memset(&st, 0, sizeof st);
    rc = esl_stats(svc, "telemetry", &st);
    if (rc != ESL_OK) {
        fprintf(stderr, "esl_stats failed: rc=%d\n", rc);
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
    rc = esl_stats(svc, "does-not-exist", &st);
    if (rc != ESL_ERR_UNKNOWN_STREAM) {
        fprintf(stderr, "FAIL: unknown stream returned rc=%d, expected %d\n", rc,
                ESL_ERR_UNKNOWN_STREAM);
        return 1;
    }

    /* NULL handling must be a no-op / clean error, never a crash. */
    esl_stream_free(NULL);
    esl_str_free(NULL);
    if (esl_flush(NULL, NULL) != ESL_ERR_INVALID_ARG) {
        fprintf(stderr, "FAIL: esl_flush(NULL) did not return ESL_ERR_INVALID_ARG\n");
        return 1;
    }

    esl_stream_free(s);
    esl_shutdown(svc);

    /* The core should have forwarded at least one log event (e.g. the buffer-only warning). */
    if (g_log_count == 0) {
        fprintf(stderr, "FAIL: no log events were forwarded to the callback\n");
        return 1;
    }

    printf("C smoke test PASSED (%d records appended, buffered, stats read back; %d log events)\n",
           N, g_log_count);
    return 0;
}
