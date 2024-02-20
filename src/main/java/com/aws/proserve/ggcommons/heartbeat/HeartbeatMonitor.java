package com.aws.proserve.ggcommons.heartbeat;

import com.aws.proserve.ggcommons.config.HeartbeatConfiguration;
import com.google.gson.JsonObject;
import oshi.PlatformEnum;
import oshi.SystemInfo;
import oshi.software.os.OSProcess;


public class HeartbeatMonitor
{
    HeartbeatConfiguration heartbeatConfiguration;
    SystemInfo si;
    OSProcess currentProc;
    OSProcess previousProc;
    long previousCpuTime = 0L;
    public HeartbeatMonitor(HeartbeatConfiguration hbConfig)
    {
        heartbeatConfiguration = hbConfig;
        si = new SystemInfo();
        currentProc = new SystemInfo().getOperatingSystem().getCurrentProcess();
    }

    public JsonObject getStats()
    {
        JsonObject data = new JsonObject();
        updateMetrics();

        JsonObject cpuData = getCpuUsage();
        if (cpuData != null)
            data.add("cpu", cpuData);

        JsonObject memData = getMemoryUsage();
        if (memData != null)
            data.add("memory", memData);

        JsonObject diskData = getDiskUsage();
        if (diskData != null)
            data.add("disk", diskData);

        JsonObject threadData = getThreadCount();
        if (threadData != null)
            data.add("threads", threadData);

        JsonObject fileData = getFileCount();
        if (fileData != null)
            data.add("files", fileData);

        return data;
    }

    private JsonObject getCpuUsage()
    {
        JsonObject retVal = null;
        if (heartbeatConfiguration.includeCpu())
        {
            retVal = new JsonObject();
            double cpuUsage;
            cpuUsage = currentProc.getProcessCpuLoadBetweenTicks(previousProc)*100;
            if (SystemInfo.getCurrentPlatform() == PlatformEnum.WINDOWS)
                cpuUsage /= si.getHardware().getProcessor().getLogicalProcessorCount();
            retVal.addProperty("cpu_usage", cpuUsage);
        }
        return retVal;
    }

    private JsonObject getMemoryUsage()
    {
        JsonObject retVal = null;
        if (heartbeatConfiguration.includeMemory())
        {
            retVal = new JsonObject();
            retVal.addProperty("memory_usage", currentProc.getResidentSetSize()/1000000);
        }
        return retVal;
    }

    private JsonObject getDiskUsage()
    {
        JsonObject retVal = null;
        if (heartbeatConfiguration.includeDisk())
        {
            retVal = new JsonObject();
        }
        return retVal;
    }

    private JsonObject getThreadCount()
    {
        JsonObject retVal = null;
        if (heartbeatConfiguration.includeThreads())
        {
            retVal = new JsonObject();
            retVal.addProperty("threads", currentProc.getThreadCount());
        }
        return retVal;
    }

    private JsonObject getFileCount()
    {
        JsonObject retVal = null;
        if (heartbeatConfiguration.includeFiles())
        {
            retVal = new JsonObject();
            retVal.addProperty("files", currentProc.getOpenFiles());
        }
        return retVal;
    }

    public void updateMetrics()
    {
        previousProc = currentProc;
        currentProc = new SystemInfo().getOperatingSystem().getCurrentProcess();
    }
}
