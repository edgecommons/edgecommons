package com.aws.proserve.ggcommons.metrics.targets;

import com.aws.proserve.ggcommons.config.ConfigManager;
import com.aws.proserve.ggcommons.metrics.Measure;
import com.aws.proserve.ggcommons.metrics.Metric;
import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.Map;


public class Log extends MetricTarget
{
    private final static Logger LOGGER = LogManager.getLogger(MetricTarget.class);
    private final Logger metricLogger;

    public Log(ConfigManager configManager)
    {
        super(configManager);
        metricLogger = LogManager.getLogger("metric");
    }

    @Override
    public void emitMetric(Metric metric, Map<String, Float> measureValues)
    {
        emitMetricNow(metric, measureValues);
    }

    @Override
    public void emitMetricNow(Metric metric, Map<String, Float> measureValues)
    {
        JsonObject metricData = buildMetricData(metric, measureValues);
        metricLogger.info(metricData.toString());
        LOGGER.trace("Metric emitted for {} emitted", metric.getName());
    }

    private JsonObject buildMetricData(Metric metric, Map<String, Float> measureValues) {
        JsonObject emfObject = new JsonObject();

        JsonObject awsObject = new JsonObject();
        awsObject.addProperty("Timestamp", System.currentTimeMillis()/1000);

        JsonArray cwMetricsArray = new JsonArray();
        JsonObject cwMetricsArrayEntry = getMetricsMetadata(metric);
        cwMetricsArray.add(cwMetricsArrayEntry);
        awsObject.add("CloudWatchMetrics", cwMetricsArray);

        for (Map.Entry<String, String> entry : metric.getDimensions().entrySet())
            emfObject.addProperty(entry.getKey(), entry.getValue());
        for (Map.Entry<String, Float> entry : measureValues.entrySet())
            emfObject.addProperty(entry.getKey(), entry.getValue());
        emfObject.add("_aws", awsObject);

        return emfObject;
    }

    private JsonObject getMetricsMetadata(Metric metric)
    {
        JsonObject cwMetricsArrayEntry = new JsonObject();
        cwMetricsArrayEntry.addProperty("Namespace", metricConfig.getNamespace());
        JsonArray dimensionSetArray = new JsonArray();
        JsonArray dimensionArray = new JsonArray();
        for (Map.Entry<String, String> dimension : metric.getDimensions().entrySet())
            dimensionArray.add(dimension.getKey());
        dimensionSetArray.add(dimensionArray);
        cwMetricsArrayEntry.add("Dimensions", dimensionSetArray);
        JsonArray metricsMetadataArray = new JsonArray();
        for (Measure measure : metric.getMeasures().values())
        {
            JsonObject measureMetadataEntry = new JsonObject();
            measureMetadataEntry.addProperty("Name", measure.getName());
            measureMetadataEntry.addProperty("Unit", measure.getUnit());
            measureMetadataEntry.addProperty("StorageResolution", measure.getStorageResolution());
            metricsMetadataArray.add(measureMetadataEntry);
        }
        cwMetricsArrayEntry.add("Metrics", metricsMetadataArray);
        return cwMetricsArrayEntry;
    }
}

