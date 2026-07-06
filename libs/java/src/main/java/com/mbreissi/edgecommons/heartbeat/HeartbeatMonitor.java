/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.heartbeat;

import com.mbreissi.edgecommons.config.HeartbeatConfiguration;
import com.google.gson.JsonObject;
import oshi.PlatformEnum;
import oshi.SystemInfo;
import oshi.software.os.OSProcess;


/**
 * Monitors and tracks the heartbeat status of a Greengrass component.
 * Manages heartbeat statistics, health checks, and status reporting.
 */
public class HeartbeatMonitor
{
    HeartbeatConfiguration heartbeatConfiguration;
    SystemInfo si;
    OSProcess currentProc;
    OSProcess previousProc;
    long previousCpuTime = 0L;
    /**
     * Creates a new HeartbeatMonitor with the specified configuration.
     *
     * @param hbConfig The heartbeat configuration settings to use
     */
    public HeartbeatMonitor(HeartbeatConfiguration hbConfig)
    {
        heartbeatConfiguration = hbConfig;
        si = new SystemInfo();
        currentProc = new SystemInfo().getOperatingSystem().getCurrentProcess();
    }

    /**
     * Gets the current health statistics for this component.
     * Collects various metrics based on configuration settings.
     *
     * @return JsonObject containing health statistics
     */
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

        JsonObject fdData = getFdCount();
        if (fdData != null)
            data.add("fds", fdData);

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
            // Mirror the Python/Rust libs: report the filesystem holding the parent of the working
            // dir, in Gigabytes (decimal, /1e9). used = total - free.
            java.io.File dir = new java.io.File("..");
            long total = dir.getTotalSpace();
            long free = dir.getFreeSpace();
            long used = total - free;
            retVal.addProperty("disk_total", total / 1.0e9);
            retVal.addProperty("disk_used", used / 1.0e9);
            retVal.addProperty("disk_free", free / 1.0e9);
        }
        return retVal;
    }

    private JsonObject getFdCount()
    {
        JsonObject retVal = null;
        if (heartbeatConfiguration.includeFds())
        {
            retVal = new JsonObject();
            // Open file descriptors via the Unix OS MXBean (Linux/macOS — the deploy target);
            // absent on Windows, where -1 signals "unavailable" rather than a bogus count.
            long fds = -1L;
            java.lang.management.OperatingSystemMXBean os =
                    java.lang.management.ManagementFactory.getOperatingSystemMXBean();
            if (os instanceof com.sun.management.UnixOperatingSystemMXBean unix)
            {
                fds = unix.getOpenFileDescriptorCount();
            }
            retVal.addProperty("fds", fds);
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
