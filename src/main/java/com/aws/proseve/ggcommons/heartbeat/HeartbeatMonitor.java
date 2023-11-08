package com.aws.proseve.ggcommons.heartbeat;


import com.github.cliftonlabs.json_simple.JsonObject;
import com.aws.proseve.ggcommons.config.HeartbeatConfiguration;
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
            data.put("cpu", cpuData);

        JsonObject memData = getMemoryUsage();
        if (memData != null)
            data.put("memory", memData);

        JsonObject diskData = getDiskUsage();
        if (diskData != null)
            data.put("disk", diskData);

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
            retVal.put("cpu_usage(%)", cpuUsage);
        }
        return retVal;
    }

    private JsonObject getMemoryUsage()
    {
        JsonObject retVal = null;
        if (heartbeatConfiguration.includeMemory())
        {
            retVal = new JsonObject();
            retVal.put("memory_usage(MB)", currentProc.getResidentSetSize()/1000000);
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

    public void updateMetrics()
    {
        previousProc = currentProc;
        currentProc = new SystemInfo().getOperatingSystem().getCurrentProcess();
    }
}
