package com.aws.proserve.ggcommons.metrics.targets;

import com.aws.proserve.ggcommons.config.ConfigManager;
import com.aws.proserve.ggcommons.metrics.Measure;
import com.aws.proserve.ggcommons.metrics.Metric;
import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import java.io.FileWriter;
import java.io.IOException;
import java.util.Map;


public class Log extends MetricTarget
{

    private final String logFileName;
    private FileWriter metricLogFile;

    public Log(ConfigManager configManager)
    {
        super(configManager);
        logFileName = configManager.resolveTemplate(metricConfig.getLogFileNameTemplate());
        try
        {
            metricLogFile = new FileWriter(logFileName, true);
        }
        catch (IOException e)
        {
            LOGGER.error("Unable to open {} for metric logging.  No metrics will be written.", logFileName);
        }
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
        try
        {
            metricLogFile.append(metricData.toString()).append("\n");
            metricLogFile.flush();
        }
        catch (IOException e)
        {
            LOGGER.warn("Exception writing metric to file {}", logFileName);
        }

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

