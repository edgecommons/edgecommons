package com.aws.proserve.ggcommons.metrics.targets;

import com.aws.proserve.ggcommons.metrics.Measure;
import com.aws.proserve.ggcommons.metrics.Metric;
import com.google.gson.JsonArray;
import com.google.gson.JsonObject;

import java.util.Map;

public class EmfHelper
{
    public static JsonObject buildMetricData(String namespace, Metric metric, Map<String,
                                             Float> measureValues, boolean largeFleetWorkaround) {
        JsonObject emfObject = new JsonObject();

        JsonObject awsObject = new JsonObject();
        awsObject.addProperty("Timestamp", System.currentTimeMillis()/1000);

        JsonArray cwMetricsArray = new JsonArray();
        JsonObject cwMetricsArrayEntry = getMetricsMetadata(namespace, metric);
        cwMetricsArray.add(cwMetricsArrayEntry);
        awsObject.add("CloudWatchMetrics", cwMetricsArray);

        for (Map.Entry<String, String> entry : metric.getDimensions().entrySet())
        {
            if (largeFleetWorkaround && entry.getKey().equals("coreName"))
                emfObject.addProperty(entry.getKey(), "ALL");
            else
                emfObject.addProperty(entry.getKey(), entry.getValue());
        }
        for (Map.Entry<String, Float> entry : measureValues.entrySet())
            emfObject.addProperty(entry.getKey(), entry.getValue());
        emfObject.add("_aws", awsObject);

        return emfObject;
    }

    private static JsonObject getMetricsMetadata(String namespace, Metric metric)
    {
        JsonObject cwMetricsArrayEntry = new JsonObject();
        cwMetricsArrayEntry.addProperty("Namespace", namespace);
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
