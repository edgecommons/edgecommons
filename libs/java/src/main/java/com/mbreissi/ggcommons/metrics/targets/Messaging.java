/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.metrics.targets;

import com.mbreissi.ggcommons.config.ConfigManager;
import com.mbreissi.ggcommons.messaging.Message;
import com.mbreissi.ggcommons.messaging.MessageBuilder;
import com.mbreissi.ggcommons.messaging.MessageIdentity;
import com.mbreissi.ggcommons.messaging.MessagingClient;
import com.mbreissi.ggcommons.metrics.Metric;
import com.mbreissi.ggcommons.uns.Uns;
import com.mbreissi.ggcommons.uns.UnsClass;
import com.google.gson.JsonObject;
import software.amazon.awssdk.aws.greengrass.model.QOS;

import java.util.Map;

/**
 * The {@code messaging} metric target (UNS-CANONICAL-DESIGN §4.3): publishes each metric to the
 * library-owned UNS metric topic {@code ecv1/{device}/{component}/main/metric/{metricName}} (the
 * metric name sanitized as a channel token) through the privileged
 * {@link com.mbreissi.ggcommons.messaging.ReservedPublisher} seam — the {@code metric} class is
 * reserved. {@code metricEmission.targetConfig.destination} still selects local/IPC vs IoT Core
 * (D-U9); the legacy {@code targetConfig.topic} override is removed.
 */
public final class Messaging extends MetricTarget {

    private boolean sendToIpc = true;
    private MessagingClient messagingService;
    /** WARN-once flag for the no-resolved-identity (test/subclass bring-up) case. */
    private boolean warnedNoIdentity = false;

    public Messaging(ConfigManager configManager) {
        super(configManager);
        this.sendToIpc = !isIotCoreDestination(metricConfig.getDestination());
    }

    /**
     * IoT Core is selected only by "iot_core"/"iotcore"; everything else (the
     * canonical "ipc", the legacy "local", and any unrecognized value) uses the
     * local/IPC transport, so a metric never routes to a possibly-unconfigured
     * IoT Core. Matches the heartbeat destination and the Python/Rust metric targets.
     */
    private static boolean isIotCoreDestination(String destination) {
        return destination.equalsIgnoreCase("iot_core") || destination.equalsIgnoreCase("iotcore");
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
            messagingService.reservedPublisher().publishToIoTCore(topic, message, QOS.AT_LEAST_ONCE);
        LOGGER.trace("Metric emitted for {} emitted", metric);
    }

    @Override
    public boolean onConfigurationChanged()
    {
        LOGGER.info("Configuration changed. Reconfiguring metric messaging destination");
        this.sendToIpc = !isIotCoreDestination(configManager.getMetricConfig().getDestination());
        return true;
    }

    @Override
    public void emitMetric(Metric metric, Map<String, Float> measureValues)
    {
        emitMetricNow(metric, measureValues);
    }
}
