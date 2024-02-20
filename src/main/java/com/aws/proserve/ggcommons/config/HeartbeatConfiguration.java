package com.aws.proserve.ggcommons.config;


import com.google.gson.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

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
    protected static final Logger LOGGER = LogManager.getLogger(HeartbeatConfiguration.class);
    int intervalSecs = 5;
    boolean includeCpu = true;
    boolean includeMemory = true;
    boolean includeDisk = false;
    boolean includeThreads = false;
    boolean includeFiles = false;
    boolean includeFds = false;

    public HeartbeatConfiguration(JsonObject jsonConfig)
    {
        if (jsonConfig != null)
        {
            if (jsonConfig.has("intervalSecs"))
            {
                intervalSecs = (jsonConfig.get("intervalSecs").getAsBigDecimal()).intValue();
                if (intervalSecs < 1)
                    intervalSecs = 5;
            }
            if (jsonConfig.has("metric"))
            {
                JsonObject metricObj = (JsonObject) jsonConfig.get("metric");
                if (metricObj.has("cpu"))
                    includeCpu =  metricObj.get("cpu").getAsBoolean();
                if (metricObj.has("memory"))
                    includeMemory =  metricObj.get("memory").getAsBoolean();
                if (metricObj.has("disk"))
                    LOGGER.warn("Reporting of disk space not supported in ggcommons-java. Ignoring");
                if (metricObj.has("threads"))
                    includeThreads =  metricObj.get("threads").getAsBoolean();
                if (metricObj.has("files"))
                    includeFiles =  metricObj.get("files").getAsBoolean();
                if (metricObj.has("fds"))
                    LOGGER.warn("Reporting of allocated file descriptors (fds) not supported in ggcommons-java. Ignoring");
            }
        }
    }

    public JsonObject toDict()
    {
        JsonObject retVal = new JsonObject();
        retVal.addProperty("intervalSecs", intervalSecs);
        JsonObject metricObj = new JsonObject();
        metricObj.addProperty("cpu", includeCpu);
        metricObj.addProperty("memory", includeMemory);
        metricObj.addProperty("disk", includeDisk);
        metricObj.addProperty("threads", includeDisk);
        metricObj.addProperty("files", includeDisk);
        metricObj.addProperty("fds", includeFds);
        retVal.add("metric", metricObj);
        return retVal;
    }

    @Override
    public String toString()
    {
        return toDict().toString();
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

    public boolean includeThreads()
    {
        return includeThreads;
    }

    public boolean includeFiles()
    {
        return includeFiles;
    }

    public boolean includeFds() { return includeFds; }

}
