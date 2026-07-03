/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.metrics.targets;

import com.mbreissi.ggcommons.config.ConfigManager;
import com.mbreissi.ggcommons.messaging.Message;
import com.mbreissi.ggcommons.messaging.MessageBuilder;
import com.mbreissi.ggcommons.messaging.MessagingClient;
import com.mbreissi.ggcommons.metrics.Metric;
import com.google.gson.JsonObject;
import software.amazon.awssdk.aws.greengrass.model.QOS;

import java.util.Map;

public final class Messaging extends MetricTarget {

    private String topic;
    private boolean sendToIpc = true;
    private MessagingClient messagingService;

    public Messaging(ConfigManager configManager) {
        super(configManager);
        this.topic = configManager.resolveTemplate(metricConfig.getTopic());
        this.sendToIpc = !isIotCoreDestination(metricConfig.getDestination());
    }

    /**
     * IoT Core is selected only by "iot_core"/"iotcore"; everything else (the
     * canonical "ipc", the legacy "local", and any unrecognized value) uses the
     * local/IPC transport, so a metric never routes to a possibly-unconfigured
     * IoT Core. Matches the heartbeat target and the Python/Rust metric targets.
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

    private void publishMessage(Metric metric, JsonObject metricObject)
    {
        Message message = MessageBuilder.create("Metric", "1.0")
                .withPayload(metricObject)
                .withConfig(configManager)
                .build();
        if (sendToIpc)
            messagingService.publish(topic, message);
        else
            messagingService.publishToIoTCore(topic, message, QOS.AT_LEAST_ONCE);
        LOGGER.trace("Metric emitted for {} emitted", metric);
    }

    @Override
    public boolean onConfigurationChanged()
    {
        LOGGER.info("Configuration changed. Reconfiguring metric messaging topic and destination");
        this.topic = configManager.resolveTemplate(configManager.getMetricConfig().getTopic());
        this.sendToIpc = !isIotCoreDestination(configManager.getMetricConfig().getDestination());
        return true;
    }

    @Override
    public void emitMetric(Metric metric, Map<String, Float> measureValues)
    {
        emitMetricNow(metric, measureValues);
    }
}
