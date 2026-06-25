package com.breissinger.ggcommons.credentials;

import java.util.HashMap;
import java.util.Map;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;

import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import com.breissinger.ggcommons.config.ConfigManager;
import com.breissinger.ggcommons.metrics.MetricBuilder;
import com.breissinger.ggcommons.metrics.MetricEmitter;

/**
 * Periodically surfaces non-sensitive credential-subsystem {@link CredentialStats} through the
 * component's {@link MetricEmitter}, so credential metrics land in the same configured target
 * (CloudWatch / messaging / log) as heartbeat and the rest. Mirrors the Rust
 * {@code CredentialMetricsBridge} and the Java {@code StreamMetricsBridge}.
 *
 * <p>Emits one metric named {@code credentials} every 30s. <b>Never emits secret values.</b>
 */
public final class CredentialMetricsBridge implements AutoCloseable {

    private static final Logger LOGGER = LogManager.getLogger(CredentialMetricsBridge.class);
    private static final long DEFAULT_INTERVAL_SECS = 30;
    private static final String METRIC = "credentials";

    private final CredentialService creds;
    private final MetricEmitter metrics;
    private final ScheduledExecutorService scheduler =
            Executors.newSingleThreadScheduledExecutor(r -> {
                Thread t = new Thread(r, "CredentialMetrics-scheduler");
                t.setDaemon(true);
                return t;
            });
    private final ScheduledFuture<?> task;

    public CredentialMetricsBridge(ConfigManager config, MetricEmitter metrics, CredentialService creds) {
        this(config, metrics, creds, DEFAULT_INTERVAL_SECS);
    }

    public CredentialMetricsBridge(ConfigManager config, MetricEmitter metrics, CredentialService creds,
                                   long intervalSecs) {
        this.creds = creds;
        this.metrics = metrics;
        int resolution = intervalSecs < 60 ? 1 : 60;
        metrics.defineMetric(MetricBuilder.create(METRIC)
                .withConfig(config)
                .addMeasure("secretCount", "Count", resolution)
                .addMeasure("lastSyncAgeMs", "Milliseconds", resolution)
                .addMeasure("syncFailures", "Count", resolution)
                .addMeasure("rotations", "Count", resolution)
                .build());
        task = scheduler.scheduleAtFixedRate(this::tick, intervalSecs, intervalSecs, TimeUnit.SECONDS);
        LOGGER.info("Credential metrics bridge started at {}s interval", intervalSecs);
    }

    /** Emit current credential stats. Guarded — telemetry-about-telemetry must never crash. */
    private void tick() {
        try {
            CredentialStats s = creds.stats();
            Map<String, Float> values = new HashMap<>(4);
            values.put("secretCount", (float) s.secretCount());
            values.put("lastSyncAgeMs", (float) (s.lastSyncAgeMs() == null ? 0 : s.lastSyncAgeMs()));
            values.put("syncFailures", (float) s.syncFailures());
            values.put("rotations", (float) s.rotations());
            metrics.emitMetric(METRIC, values);
        } catch (Exception e) {
            LOGGER.debug("failed to emit credential stats: {}", e.getMessage());
        }
    }

    @Override
    public void close() {
        task.cancel(false);
        scheduler.shutdownNow();
    }
}
