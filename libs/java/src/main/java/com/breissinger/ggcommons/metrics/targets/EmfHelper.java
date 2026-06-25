/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons.metrics.targets;

import com.breissinger.ggcommons.metrics.Measure;
import com.breissinger.ggcommons.metrics.Metric;
import com.google.gson.JsonArray;
import com.google.gson.JsonObject;

import java.util.Map;

public class EmfHelper
{
    public static JsonObject buildMetricData(String namespace, Metric metric, Map<String,
                                             Float> measureValues, boolean largeFleetWorkaround) {
        JsonObject emfObject = new JsonObject();

        JsonObject awsObject = new JsonObject();
        awsObject.addProperty("Timestamp", System.currentTimeMillis());

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
            measureMetadataEntry.addProperty("Name", measure.name());
            measureMetadataEntry.addProperty("Unit", measure.unit());
            measureMetadataEntry.addProperty("StorageResolution", measure.storageResolution());
            metricsMetadataArray.add(measureMetadataEntry);
        }
        cwMetricsArrayEntry.add("Metrics", metricsMetadataArray);
        return cwMetricsArrayEntry;
    }

}
