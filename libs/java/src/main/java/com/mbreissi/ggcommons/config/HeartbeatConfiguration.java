/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.config;


import com.google.gson.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

//{
//    "enabled": true,
//    "intervalSecs": 5,
//    "measures": {
//        "cpu": true,
//        "memory": true
//        "disk": false
//    },
//    "destination": "local"
//}

/**
 * Configuration model for the component heartbeat (UNS-CANONICAL-DESIGN §4.3, D-U14/D-U20).
 *
 * <p>The heartbeat is a library-owned UNS {@code state} keepalive published each tick to
 * {@code ecv1/{device}/{component}/main/state} (body {@code {"status":"RUNNING","uptimeSecs":n}},
 * best-effort {@code {"status":"STOPPED"}} on graceful shutdown), with the enabled system measures
 * emitted as the metric {@code sys} through the normal metric subsystem. The legacy
 * {@code targets[]} array (the heartbeat topic-override drift knobs) is removed — hard cut;
 * {@link #getDestination()} governs only the state keepalive's transport ({@code local} vs
 * {@code iotcore}); the measures route through the metric subsystem's own target.
 */
public class HeartbeatConfiguration
{
    protected static final Logger LOGGER = LogManager.getLogger(HeartbeatConfiguration.class);

    /** The schema default for {@code heartbeat.destination} — the local/IPC transport. */
    public final static String DEFAULT_DESTINATION = "local";

    boolean enabled = true;
    int intervalSecs = 5;
    boolean includeCpu = true;
    boolean includeMemory = true;
    boolean includeDisk = false;
    boolean includeThreads = false;
    boolean includeFiles = false;
    boolean includeFds = false;
    String destination = DEFAULT_DESTINATION;

    /**
     * Creates a new heartbeat configuration from a JSON configuration object.
     *
     * @param jsonConfig The JSON object containing heartbeat settings
     */
    public HeartbeatConfiguration(JsonObject jsonConfig)
    {
        if (jsonConfig != null)
        {
            if (jsonConfig.has("enabled"))
            {
                enabled = jsonConfig.get("enabled").getAsBoolean();
            }
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
                    includeDisk =  metricObj.get("disk").getAsBoolean();
                if (metricObj.has("threads"))
                    includeThreads =  metricObj.get("threads").getAsBoolean();
                if (metricObj.has("files"))
                    includeFiles =  metricObj.get("files").getAsBoolean();
                if (metricObj.has("fds"))
                    includeFds =  metricObj.get("fds").getAsBoolean();
            }
            if (jsonConfig.has("destination"))
            {
                destination = jsonConfig.get("destination").getAsString();
            }
        }
    }

    public JsonObject toDict()
    {
        JsonObject retVal = new JsonObject();
        retVal.addProperty("enabled", enabled);
        retVal.addProperty("intervalSecs", intervalSecs);
        JsonObject metricObj = new JsonObject();
        metricObj.addProperty("cpu", includeCpu);
        metricObj.addProperty("memory", includeMemory);
        metricObj.addProperty("disk", includeDisk);
        metricObj.addProperty("threads", includeThreads);
        metricObj.addProperty("files", includeFiles);
        metricObj.addProperty("fds", includeFds);
        retVal.add("measures", metricObj);
        retVal.addProperty("destination", destination);
        return retVal;
    }

    @Override
    public String toString()
    {
        return toDict().toString();
    }

    /**
     * Whether the heartbeat (state keepalive + {@code sys} measures metric) runs. Default
     * {@code true} — on / 5 s / local (D-U14).
     */
    public boolean isEnabled()
    {
        return enabled;
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

    /**
     * The publish destination of the {@code state} keepalive only — {@code "local"} (the
     * local/IPC transport, the default) or {@code "iotcore"} (AWS IoT Core). The measures route
     * through the metric subsystem's own target and are unaffected.
     */
    public String getDestination()
    {
        return destination;
    }

}
