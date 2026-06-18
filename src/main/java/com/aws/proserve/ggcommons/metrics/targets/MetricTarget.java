/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.metrics.targets;

import com.aws.proserve.ggcommons.config.ConfigManager;
import com.aws.proserve.ggcommons.config.ConfigurationChangeListener;
import com.aws.proserve.ggcommons.config.MetricConfiguration;
import com.aws.proserve.ggcommons.metrics.Metric;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.Map;

public abstract class MetricTarget implements ConfigurationChangeListener
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

    /** Flushes any buffered metrics to the target. Default no-op (targets that don't buffer). */
    public void flush() {}

    /** Releases any resources held by this target (timers, clients, appenders). Default no-op. */
    public void close() {}
}
