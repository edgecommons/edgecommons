/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons.metrics.targets;

import com.breissinger.ggcommons.config.ConfigManager;
import com.breissinger.ggcommons.config.ConfigurationChangeListener;
import com.breissinger.ggcommons.config.MetricConfiguration;
import com.breissinger.ggcommons.metrics.Metric;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.Map;

public abstract sealed class MetricTarget implements ConfigurationChangeListener
        permits CloudWatch, CloudWatchComponent, Messaging, Log, Prometheus
{

    protected static final Logger LOGGER = LogManager.getLogger(MetricTarget.class);

    protected final ConfigManager configManager;
    protected final MetricConfiguration metricConfig;

    MetricTarget(ConfigManager configManager)
    {
        this.configManager = configManager;
        this.metricConfig = configManager.getMetricConfig();
    }

    public abstract void emitMetric(Metric metric, Map<String, Float> measureValues);

    public abstract void emitMetricNow(Metric metric, Map<String, Float> measureValues);

    @Override
    public abstract boolean onConfigurationChanged();

    /**
     * Flushes any buffered metrics to the target. Default no-op (targets that don't buffer).
     *
     * <p><b>Lifecycle note (FR-MET-2):</b> the push targets (log/messaging/cloudwatch/
     * cloudwatchcomponent) deliver on {@code emit*}/{@code flush}. The pull-based {@link Prometheus}
     * target <em>inverts</em> this: {@code emit*} only update an in-process registry and {@code flush()}
     * is a no-op w.r.t. delivery — a scrape of {@code /metrics} pulls the current values. A caller
     * relying on {@code flush}/{@code emitMetricNow} for flush-before-exit gets nothing delivered until
     * the next scrape under the prometheus target.
     */
    public void flush() {}

    /**
     * Releases any resources held by this target (timers, clients, appenders). Default no-op. For the
     * {@link Prometheus} target this stops the embedded HTTP listener and releases its port/thread.
     */
    public void close() {}
}
