package com.aws.proserve.ggcommons.heartbeat;

import com.aws.proserve.ggcommons.config.ConfigManager;
import com.aws.proserve.ggcommons.config.ConfigurationChangeListener;

import com.aws.proserve.ggcommons.metrics.Measure;
import com.aws.proserve.ggcommons.metrics.Metric;
import com.aws.proserve.ggcommons.metrics.MetricEmitter;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.HashMap;
import java.util.Map;
import java.util.Timer;
import java.util.TimerTask;

public class Heartbeat implements ConfigurationChangeListener
{
    protected static final Logger LOGGER = LogManager.getLogger(Heartbeat.class);
    private final ConfigManager configManager;
    private HeartbeatMonitor heartbeatMonitor;
    private Timer heartbeatTimer;

    public Heartbeat(ConfigManager config)
    {
        configManager = config;
        configManager.addConfigChangeListener(this);
        defineMetric();
        initHeartbeat();
    }

    private void initHeartbeat()
    {
        heartbeatMonitor = new HeartbeatMonitor(configManager.getHeartbeatConfig());
        heartbeatTimer = new Timer("Heartbeat timer", true);
        heartbeatTimer.scheduleAtFixedRate(new Heartbeater(), 0, configManager.getHeartbeatConfig().getIntervalSecs()*1000L);
        LOGGER.info("Heartbeat initialized at {} second interval", configManager.getHeartbeatConfig().getIntervalSecs());
    }

    private void defineMetric()
    {
        int storageResolution = configManager.getHeartbeatConfig().getIntervalSecs() < 60 ? 1 : 60;
        Metric metric = new Metric("heartbeat");
        metric.addMeasure(new Measure("disk", "Gigabytes", storageResolution));
        metric.addMeasure(new Measure("cpu_usage", "Percent", storageResolution));
        metric.addMeasure(new Measure("memory_usage", "Megabytes", storageResolution));
        metric.addMeasure(new Measure("threads", "Count", storageResolution));
        metric.addMeasure(new Measure("files", "Count", storageResolution));
        metric.addMeasure(new Measure("fds", "Count", storageResolution));
        MetricEmitter.defineMetric(metric);
    }

    private void publishHeartbeat()
    {
        JsonObject data = heartbeatMonitor.getStats();
        Map<String, Float> measureValues = new HashMap<>();
        for (Map.Entry<String, JsonElement> entry : data.entrySet())
        {
            for (String measureName : entry.getValue().getAsJsonObject().keySet())
            {
                measureValues.put(measureName, entry.getValue().getAsJsonObject().get(measureName).getAsFloat());
            }
        }
        MetricEmitter.emitMetricNow("heartbeat", measureValues);
    }

    @Override
    public boolean onConfigurationChanged()
    {
        LOGGER.info("Configuration changed, restarting heartbeat");
        if (heartbeatTimer != null)
        {
            heartbeatTimer.cancel();
            heartbeatTimer.purge();
        }
        initHeartbeat();
        return true;
    }

    private class Heartbeater extends TimerTask
    {
        @Override
        public void run()
        {
            publishHeartbeat();
        }
    }
}
