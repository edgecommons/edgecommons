package com.aws.proserve.ggcommons.metrics;

import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import software.amazon.awssdk.services.cloudwatch.model.Dimension;

import java.util.ArrayList;
import java.util.Collection;
import java.util.HashMap;
import java.util.Map;

public class Metric
{
    private final String name;

    private final String namespace;
    private final Map<String, Measure> measures;

    private final Map<String, String> dimensions;

    public Metric(String name)
    {
        this(name, MetricEmitter.getMetricConfig().getNamespace(), new HashMap<>(), new HashMap<>());
    }

    public Metric(String name, String namespace, Map<String, Measure> measures, Map<String, String> dimensions)
    {
        this.name = name;
        this.namespace = namespace;
        this.measures = measures;
        this.dimensions = dimensions;
        addDimension("coreName", MetricEmitter.getThingName());
        addDimension("category", name);
        addDimension("component", MetricEmitter.getComponentName());
    }

    public void addMeasure(Measure measure)
    {
        measures.put(measure.getName(), measure);
    }

    public void addDimension(String name, String value)
    {
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
