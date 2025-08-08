/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.metrics.targets;

import com.aws.proserve.ggcommons.config.ConfigManager;
import com.aws.proserve.ggcommons.interfaces.IConfigurationService;
import com.aws.proserve.ggcommons.interfaces.IMessagingService;
import com.aws.proserve.ggcommons.messaging.Message;
import com.aws.proserve.ggcommons.messaging.MessageBuilder;
import com.aws.proserve.ggcommons.messaging.MessagingClient;
import com.aws.proserve.ggcommons.metrics.Metric;
import com.google.gson.JsonObject;
import software.amazon.awssdk.aws.greengrass.model.QOS;

import java.util.Map;

public class Messaging extends MetricTarget {

    private String topic;
    private boolean sendToIpc = true;
    private IMessagingService messagingService;
    private IConfigurationService configService;

    /**
     * @deprecated Use {@link #Messaging(IConfigurationService)} instead
     */
    @Deprecated
    public Messaging(ConfigManager configManager) {
        this((IConfigurationService) configManager);
    }
    
    public Messaging(IConfigurationService configService) {
        super(configService);
        this.configService = configService;
        this.topic = configService.resolveTemplate(metricConfig.getTopic());
        this.sendToIpc = metricConfig.getDestination().equalsIgnoreCase("ipc");
    }
    
    public void setMessagingService(IMessagingService messagingService) {
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
                .withConfig(configService)
                .build();
        if (sendToIpc)
            messagingService.publish(topic, message);
        else
            messagingService.publishToIotCore(topic, message, QOS.AT_LEAST_ONCE);
        LOGGER.trace("Metric emitted for {} emitted", metric);
    }

    @Override
    public boolean onConfigurationChanged()
    {
        LOGGER.info("Configuration changed. Reconfiguring metric messaging topic and destination");
        this.topic = configService.resolveTemplate(configService.getMetricConfig().getTopic());
        this.sendToIpc = configService.getMetricConfig().getDestination().equalsIgnoreCase("ipc");
        return true;
    }

    @Override
    public void emitMetric(Metric metric, Map<String, Float> measureValues)
    {
        emitMetricNow(metric, measureValues);
    }
}