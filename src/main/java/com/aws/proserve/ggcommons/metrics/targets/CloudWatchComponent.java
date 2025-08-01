/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.metrics.targets;

import com.aws.proserve.ggcommons.config.ConfigManager;
import com.aws.proserve.ggcommons.interfaces.IConfigurationService;
import com.aws.proserve.ggcommons.interfaces.IMessagingService;
import com.aws.proserve.ggcommons.messaging.MessagingClient;
import com.aws.proserve.ggcommons.metrics.Metric;
import com.google.gson.JsonObject;

import java.util.Map;

// NOTE: The CloudWatchComponent target does not honor the "largeFleetWorkaround" configuration flag due to
//       limitations in the Greengrass CloudWatch Metrics component (the coreName dimension is implicit and
//       therefore cannot be overridden)

public class CloudWatchComponent extends MetricTarget
{
    private String topic;
    private IMessagingService messagingService;

    /**
     * @deprecated Use {@link #CloudWatchComponent(IConfigurationService)} instead
     */
    @Deprecated
    public CloudWatchComponent(ConfigManager configManager) {
        this((IConfigurationService) configManager);
    }
    
    public CloudWatchComponent(IConfigurationService configService) {
        super(configService);
        this.topic = configService.resolveTemplate(metricConfig.getTopic());
    }
    
    public void setMessagingService(IMessagingService messagingService) {
        this.messagingService = messagingService;
    }

    @Override
    public void emitMetricNow(Metric metric, Map<String, Float> measureValues)
    {
        for (Map.Entry<String,Float> entry : measureValues.entrySet())
        {
            JsonObject metricObject = buildMetricData(metric, entry.getKey(), entry.getValue());
            if (messagingService != null) {
                messagingService.publishRaw(topic, metricObject);
            } else {
                MessagingClient.publishRaw(topic, metricObject);
            }
            LOGGER.trace("Metric emitted for {} emitted", metric);
        }
    }

    @Override
    public boolean onConfigurationChanged()
    {
        LOGGER.info("Configuration changed. Reconfiguring CloudWatch Component topic");
        this.topic = configService.resolveTemplate(configManager.getMetricConfig().getTopic());
        return false;
    }

    @Override
    public void emitMetric(Metric metric, Map<String, Float> measureValues)
    {
        emitMetricNow(metric, measureValues);
    }

    private JsonObject buildMetricData(Metric metric, String measureName, Float measureValue)
    {
        JsonObject retVal = new JsonObject();

        JsonObject requestObject = new JsonObject();
        requestObject.addProperty("namespace", metric.getNamespace());

        JsonObject metricData = new JsonObject();
        metricData.addProperty("metricName", measureName);
        metricData.addProperty("timestamp", System.currentTimeMillis()/1000);
        metricData.addProperty("value", measureValue);
        metricData.addProperty("unit", metric.getMeasure(measureName).getUnit());
        metricData.add("dimensions", metric.dimensionsAsJson(false));

        requestObject.add("metricData", metricData);
        retVal.add("request", requestObject);
        return retVal;
    }

}