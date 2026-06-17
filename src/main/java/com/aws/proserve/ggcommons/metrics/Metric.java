/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.metrics;

import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import software.amazon.awssdk.services.cloudwatch.model.Dimension;

import java.util.ArrayList;
import java.util.Collection;
import java.util.HashMap;
import java.util.Map;

/**
 * Represents a metric definition in the Greengrass metrics system.
 * Contains metric configuration including name, namespace, dimensions, and measures.
 */
public class Metric
{
    private final String name;
    private final String namespace;
    private final Map<String, Measure> measures;
    private final Map<String, String> dimensions;

    /**
     * Creates a new metric with complete configuration.
     *
     * @param name The name of the metric
     * @param namespace The metric namespace
     * @param measures The map of measure definitions
     * @param dimensions The map of dimension key-values
     */
    public Metric(String name, String namespace, Map<String, Measure> measures, Map<String, String> dimensions)
    {
        if (measures == null) {
            throw new IllegalArgumentException("Measures cannot be null. At least 1 measure must be defined for a metric.");
        }
        
        this.name = name;
        this.namespace = namespace;
        this.measures = measures;
        this.dimensions = dimensions != null ? dimensions : new HashMap<>();
        addDimension("category", name);
    }

    /**
     * Adds a measure to this metric.
     *
     * @param measure The measure to add
     */
    public void addMeasure(Measure measure)
    {
        measures.put(measure.getName(), measure);
    }

    public void addDimension(String name, String value)
    {
        if (!dimensions.containsKey(name) && dimensions.size() >= 10) {
            throw new IllegalArgumentException("Maximum of 10 dimensions allowed per metric");
        }
        dimensions.put(name, value);
    }

    public JsonArray dimensionsAsJson()
    {
        return dimensionsAsJson(true);
    }

    public JsonArray dimensionsAsJson(boolean includeCoreName)
    {
        JsonArray jsonArray = new JsonArray();
        for (Map.Entry<String, String> dimension : dimensions.entrySet())
        {
            if (dimension.getKey().equals("coreName"))
            {
                if (includeCoreName)
                {
                    JsonObject arrayElem = new JsonObject();
                    arrayElem.addProperty("name", dimension.getKey());
                    arrayElem.addProperty("value", dimension.getValue());
                    jsonArray.add(arrayElem);
                }
            }
            else
            {
                JsonObject arrayElem = new JsonObject();
                arrayElem.addProperty("name", dimension.getKey());
                arrayElem.addProperty("value", dimension.getValue());
                jsonArray.add(arrayElem);
            }
        }
        return jsonArray;
    }

    public Collection<Dimension> dimensionsAsCollection(boolean largeFleetWorkaround)
    {
        Collection<Dimension> retVal = new ArrayList<>();
        for (Map.Entry<String, String> entry : dimensions.entrySet())
        {
            Dimension dimension;
            if (entry.getKey().equals("coreName") && largeFleetWorkaround)
            {
                dimension = Dimension.builder()
                                     .name(entry.getKey())
                                     .value("ALL")
                                     .build();
            }
            else
            {
                dimension = Dimension.builder()
                                     .name(entry.getKey())
                                     .value(entry.getValue())
                                     .build();
            }
            retVal.add(dimension);
        }
        return retVal;
    }

    public Collection<Dimension> dimensionsAsCollection()
    {
        return dimensionsAsCollection(false);
    }

    /**
     * Gets the name of this metric.
     *
     * @return The metric name
     */
    public String getName()
    {
        return name;
    }

    public String getNamespace()
    {
        return namespace;
    }

    public Map<String, Measure> getMeasures()
    {
        return measures;
    }

    public Measure getMeasure(String name)
    {
        return measures.get(name);
    }

    public Map<String, String> getDimensions()
    {
        return dimensions;
    }
}