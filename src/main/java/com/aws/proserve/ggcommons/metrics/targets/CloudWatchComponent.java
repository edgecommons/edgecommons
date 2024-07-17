package com.aws.proserve.ggcommons.metrics.targets;

import com.aws.proserve.ggcommons.config.ConfigManager;
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

    public CloudWatchComponent(ConfigManager configManager) {
        super(configManager);
        this.topic = configManager.resolveTemplate(metricConfig.getTopic());
    }

    @Override
    public void emitMetricNow(Metric metric, Map<String, Float> measureValues)
    {
        for (Map.Entry<String,Float> entry : measureValues.entrySet())
        {
            JsonObject metricObject = buildMetricData(metric, entry.getKey(), entry.getValue());
            MessagingClient.publishRaw(topic, metricObject);
            LOGGER.trace("Metric emitted for {} emitted", metric);
        }
    }

    @Override
    public boolean onConfigurationChanged()
    {
        LOGGER.info("Configuration changed. Reconfiguring CloudWatch Component topic");
        this.topic = configManager.resolveTemplate(configManager.getMetricConfig().getTopic());
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
