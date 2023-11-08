package com.aws.proserve.ggcommons.config;

import com.github.cliftonlabs.json_simple.JsonObject;
import com.github.cliftonlabs.json_simple.Jsoner;

import java.math.BigDecimal;

//{
//    "intervalSecs": 5,
//    "metric": {
//        "cpu": true,
//        "memory": true
//        "disk": false
//    }
//}

public class HeartbeatConfiguration
{
    int intervalSecs = 5;
    boolean includeCpu = true;
    boolean includeMemory = true;
    boolean includeDisk = false;
    String topic = "heartbeat/{ThingName}/{ComponentName}";


    public HeartbeatConfiguration(JsonObject jsonConfig)
    {
        if (jsonConfig != null)
        {
            if (jsonConfig.containsKey("intervalSecs"))
            {
                intervalSecs = ((BigDecimal) jsonConfig.get("intervalSecs")).intValue();
                if (intervalSecs < 1)
                    intervalSecs = 5;
            }
            if (jsonConfig.containsKey("metric"))
            {
                JsonObject metricObj = (JsonObject) jsonConfig.get("metric");
                if (metricObj.containsKey("cpu"))
                    includeCpu = (boolean) metricObj.get("cpu");
                if (metricObj.containsKey("memory"))
                    includeMemory = (boolean) metricObj.get("memory");
                if (metricObj.containsKey("disk"))
                    includeDisk = (boolean) metricObj.get("disk");
            }
            if (jsonConfig.containsKey("topic"))
                topic = (String) jsonConfig.get("topic");
        }
    }

    public JsonObject toDict()
    {
        JsonObject retVal = new JsonObject();
        retVal.put("intervalSecs", intervalSecs);
        JsonObject metricObj = new JsonObject();
        metricObj.put("cpu", includeCpu);
        metricObj.put("memory", includeMemory);
        metricObj.put("disk", includeDisk);
        retVal.put("metric", metricObj);
        return retVal;
    }

    @Override
    public String toString()
    {
        return Jsoner.serialize(toDict());
    }

    public int getIntervalSecs()
    {
        return intervalSecs;
    }

    public boolean includeCpu()
    {
        return includeCpu;
    }

    public boolean includeMemory()
    {
        return includeMemory;
    }

    public boolean includeDisk()
    {
        return includeDisk;
    }

    public String getTopic() {
        return topic;
    }
}
