/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.metrics.targets;

import com.mbreissi.edgecommons.config.ConfigManager;
import com.mbreissi.edgecommons.messaging.Message;
import com.mbreissi.edgecommons.messaging.MessageBuilder;
import com.mbreissi.edgecommons.messaging.MessageIdentity;
import com.mbreissi.edgecommons.messaging.MessagingClient;
import com.mbreissi.edgecommons.messaging.Qos;
import com.mbreissi.edgecommons.metrics.Metric;
import com.mbreissi.edgecommons.uns.Uns;
import com.mbreissi.edgecommons.uns.UnsClass;
import com.google.gson.JsonObject;

import java.util.Map;

/**
 * The {@code messaging} metric target (UNS-CANONICAL-DESIGN §4.3): publishes each metric to the
 * library-owned UNS metric topic {@code ecv1/{device}/{component}/main/metric/{metricName}} (the
 * metric name sanitized as a channel token) through the privileged
 * {@link com.mbreissi.edgecommons.messaging.ReservedPublisher} seam — the {@code metric} class is
 * reserved. {@code metricEmission.targetConfig.destination} still selects local/IPC vs northbound
 * (D-U9); the legacy {@code targetConfig.topic} override is removed.
 */
public final class Messaging extends MetricTarget {

    private boolean sendToIpc = true;
    private MessagingClient messagingService;
    /** WARN-once flag for the no-resolved-identity (test/subclass bring-up) case. */
    private boolean warnedNoIdentity = false;

    public Messaging(ConfigManager configManager) {
        super(configManager);
        this.sendToIpc = !isNorthboundDestination(metricConfig.getDestination());
    }

    /**
     * Northbound is selected only by "northbound"; everything else (the canonical "ipc",
     * "local", and any unrecognized value) uses the local/IPC transport, so a metric never routes
     * to a possibly-unconfigured northbound broker. Matches the heartbeat destination and the
     * Python/Rust metric targets.
     */
    private static boolean isNorthboundDestination(String destination) {
        return destination != null && destination.equalsIgnoreCase("northbound");
    }

    public void setMessagingService(MessagingClient messagingService) {
        this.messagingService = messagingService;
    }

    @Override
    public void emitMetricNow(Metric metric, Map<String, Float> measureValues)
    {
        JsonObject metricObject = EmfHelper.buildMetricData(metricConfig.getNamespace(), metric, measureValues, false);
        publishMessage(metric, metricObject);

        if (metricConfig.getLargeFleetWorkaround())
        {
            metricObject = EmfHelper.buildMetricData(metricConfig.getNamespace(), metric, measureValues, true);
            publishMessage(metric, metricObject);
        }
    }

    /**
     * The metric's UNS topic — {@code ecv1[/{site}]/{device}/{component}/main/metric/{name}} with
     * the metric name passed through the template sanitizer (the §2.2 channel-token rule), or
     * {@code null} (WARN once) when no component identity is resolved (mock/test bring-up).
     */
    private String metricTopic(Metric metric)
    {
        MessageIdentity identity = configManager.getComponentIdentity();
        if (identity == null)
        {
            if (!warnedNoIdentity)
            {
                warnedNoIdentity = true;
                LOGGER.warn("No resolved component identity - the messaging metric target cannot"
                        + " build UNS metric topics; metrics are dropped");
            }
            return null;
        }
        return new Uns(identity, configManager.isTopicIncludeRoot())
                .topic(UnsClass.METRIC, ConfigManager.sanitize(metric.getName()));
    }

    private void publishMessage(Metric metric, JsonObject metricObject)
    {
        String topic = metricTopic(metric);
        if (topic == null)
        {
            return;
        }
        Message message = MessageBuilder.create("Metric", "1.0")
                .withPayload(metricObject)
                .withConfig(configManager)
                .build();
        // The metric class is reserved (§4.1) - publish through the privileged seam (§4.2).
        if (sendToIpc)
            messagingService.reservedPublisher().publish(topic, message);
        else
            messagingService.reservedPublisher().publishNorthbound(topic, message, Qos.AT_LEAST_ONCE);
        LOGGER.trace("Metric emitted for {} emitted", metric);
    }

    @Override
    public boolean onConfigurationChanged()
    {
        LOGGER.info("Configuration changed. Reconfiguring metric messaging destination");
        this.sendToIpc = !isNorthboundDestination(configManager.getMetricConfig().getDestination());
        return true;
    }

    @Override
    public void emitMetric(Metric metric, Map<String, Float> measureValues)
    {
        emitMetricNow(metric, measureValues);
    }
}
