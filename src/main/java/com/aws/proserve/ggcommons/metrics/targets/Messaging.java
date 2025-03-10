/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.metrics.targets;

import com.aws.proserve.ggcommons.config.ConfigManager;
import com.aws.proserve.ggcommons.messaging.Message;
import com.aws.proserve.ggcommons.messaging.MessagingClient;
import com.aws.proserve.ggcommons.metrics.Metric;
import com.google.gson.JsonObject;
import software.amazon.awssdk.aws.greengrass.model.QOS;

import java.util.Map;

public class Messaging extends MetricTarget {

    private String topic;
    private boolean sendToIpc = true;

    public Messaging(ConfigManager configManager) {
        super(configManager);
        this.topic = configManager.resolveTemplate(metricConfig.getTopic());
        this.sendToIpc = metricConfig.getDestination().equalsIgnoreCase("ipc");
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
        Message message = Message.buildFromConfig("Metric", "1.0", metricObject, configManager);
        if (sendToIpc)
            MessagingClient.publish(topic, message);
        else
            MessagingClient.publishToIotCore(topic, message, QOS.AT_LEAST_ONCE);
        LOGGER.trace("Metric emitted for {} emitted", metric);
    }

    @Override
    public boolean onConfigurationChanged()
    {
        LOGGER.info("Configuration changed. Reconfiguring metric messaging topic and destination");
        this.topic = configManager.resolveTemplate(configManager.getMetricConfig().getTopic());
        this.sendToIpc = configManager.getMetricConfig().getDestination().equalsIgnoreCase("ipc");
        return true;
    }

    @Override
    public void emitMetric(Metric metric, Map<String, Float> measureValues)
    {
        emitMetricNow(metric, measureValues);
    }

//    private JsonObject buildMetricData(Metric metric, Map<String, Float> measureValues) {
//        JsonObject metricData = new JsonObject();
//        metricData.addProperty("namespace", metric.getNamespace());
//        metricData.addProperty("timestamp", System.currentTimeMillis());
//        metricData.add("dimensions", metric.dimensionsAsJson());
//        JsonArray measures = new JsonArray();
//        for (Map.Entry<String, Float> entry : measureValues.entrySet())
//        {
//            JsonObject measure = new JsonObject();
//            measure.addProperty("name", entry.getKey());
//            measure.addProperty("value", entry.getValue());
//            measures.add(measure);
//        }
//        metricData.add("measures", measures);
//        return metricData;
//    }
}
