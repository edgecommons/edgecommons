/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.config;


import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.ArrayList;
import java.util.List;

//{
//    "intervalSecs": 5,
//    "measures": {
//        "cpu": true,
//        "memory": true
//        "disk": false
//    },
//    "targets": [
//        {
//            "type": "metric"
//        },
//        {
//            "type": "messaging",
//            "config": {
//                "destination": "ipc",
//                "topic": "{ThingName}/{ComponentName}/heartbeat"
//             }
//        }
//    ]
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
    final List<HeartbeatTarget> targets = new ArrayList<>();
    public final static String DEFAULT_TOPIC = "ggcommons/{ThingName}/{ComponentName}/heartbeat";
    public final static String DEFAULT_MESSAGING_DESTINATION = "ipc";

    public static class HeartbeatTarget {
        String type;
        JsonObject config;

        public String getType()
        {
            return type;
        }

        public JsonObject getConfig()
        {
            return config;
        }
    }

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
            if (jsonConfig.has("measures"))
            {
                JsonObject metricObj = (JsonObject) jsonConfig.get("measures");
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
            if (jsonConfig.has("targets"))
            {
                JsonArray targetArray = jsonConfig.get("targets").getAsJsonArray();
                for (JsonElement targetElem : targetArray)
                {
                    JsonObject targetObj = targetElem.getAsJsonObject();
                    HeartbeatTarget target = new HeartbeatTarget();
                    target.type = targetObj.get("type").getAsString();
                    if (target.type.equalsIgnoreCase("messaging") || target.type.equalsIgnoreCase("metric"))
                    {
                        if (targetObj.has("config"))
                            target.config = targetObj.get("config").getAsJsonObject();
                        targets.add(target);
                    }
                    else
                    {
                        LOGGER.warn("Unrecognized heartbeat target '{}'. Ignoring", target.type);
                    }
                }
            }
        }
        if (targets.isEmpty())
        {
            HeartbeatTarget target = new HeartbeatTarget();
            target.type = "metric";
            targets.add(target);
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
        JsonArray targetArray = new JsonArray();
        for (HeartbeatTarget target : targets)
        {
            JsonObject targetObj = new JsonObject();
            targetObj.addProperty("type", target.type);
            if (target.config != null)
                targetObj.add("config", target.config);
            targetArray.add(targetObj);
        }
        retVal.add("targets", targetArray);
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

    public List<HeartbeatTarget> getTargets()
    {
        return targets;
    }

}
