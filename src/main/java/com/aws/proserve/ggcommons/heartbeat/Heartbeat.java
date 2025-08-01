/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.heartbeat;

import com.aws.proserve.ggcommons.config.ConfigManager;
import com.aws.proserve.ggcommons.config.ConfigurationChangeListener;
import com.aws.proserve.ggcommons.config.HeartbeatConfiguration;
import com.aws.proserve.ggcommons.interfaces.IMessagingService;
import com.aws.proserve.ggcommons.interfaces.IMetricService;
import com.aws.proserve.ggcommons.messaging.Message;
import com.aws.proserve.ggcommons.messaging.MessagingClient;
import com.aws.proserve.ggcommons.metrics.Measure;
import com.aws.proserve.ggcommons.metrics.Metric;
import com.aws.proserve.ggcommons.metrics.MetricEmitter;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import software.amazon.awssdk.aws.greengrass.model.QOS;

import java.util.HashMap;
import java.util.Map;
import java.util.Timer;
import java.util.TimerTask;

/**
 * Implements heartbeat functionality for Greengrass components to monitor their health status.
 * This class periodically publishes heartbeat messages and handles configuration changes.
 */
public class Heartbeat implements ConfigurationChangeListener
{
    protected static final Logger LOGGER = LogManager.getLogger(Heartbeat.class);

    private static final String MESSAGE_NAME = "heartbeat";
    private static final String MESSAGE_VERSION = "1.0.0";
    private final ConfigManager configManager;
    private IMessagingService messagingService;
    private IMetricService metricService;
    private HeartbeatMonitor heartbeatMonitor;
    private Timer heartbeatTimer;
    private final Object timerLock = new Object();

    /**
     * Constructs a new Heartbeat instance for the component.
     *
     * @param config The configuration manager containing heartbeat settings
     */
    public Heartbeat(ConfigManager config)
    {
        configManager = config;
        configManager.addConfigChangeListener(this);
        defineMetric();
        initHeartbeat();
    }
    
    /**
     * Sets the messaging service for dependency injection.
     * 
     * @param messagingService The messaging service implementation
     */
    public void setMessagingService(IMessagingService messagingService) {
        this.messagingService = messagingService;
    }
    
    /**
     * Sets the metric service for dependency injection.
     * 
     * @param metricService The metric service implementation
     */
    public void setMetricService(IMetricService metricService) {
        this.metricService = metricService;
    }

    /**
     * Initializes the heartbeat mechanism based on the current configuration.
     * Sets up the timer for periodic heartbeat publishing.
     */
    private void initHeartbeat()
    {
        synchronized (timerLock) {
            if (heartbeatTimer != null) {
                heartbeatTimer.cancel();
                heartbeatTimer.purge();
            }
            heartbeatMonitor = new HeartbeatMonitor(configManager.getHeartbeatConfig());
            heartbeatTimer = new Timer("Heartbeat timer", true);
            heartbeatTimer.scheduleAtFixedRate(new Heartbeater(), 0, configManager.getHeartbeatConfig().getIntervalSecs()*1000L);
            LOGGER.info("Heartbeat initialized at {} second interval", configManager.getHeartbeatConfig().getIntervalSecs());
        }
    }

    /**
     * Defines the heartbeat metric in the metrics system.
     * This metric is used to track the component's health status.
     */
    private void defineMetric()
    {
        int storageResolution = configManager.getHeartbeatConfig().getIntervalSecs() < 60 ? 1 : 60;
        Metric metric = new Metric("heartbeat");
        metric.addMeasure(new Measure("disk_total", "Gigabytes", storageResolution));
        metric.addMeasure(new Measure("disk_used", "Gigabytes", storageResolution));
        metric.addMeasure(new Measure("disk_free", "Gigabytes", storageResolution));
        metric.addMeasure(new Measure("cpu_usage", "Percent", storageResolution));
        metric.addMeasure(new Measure("memory_usage", "Megabytes", storageResolution));
        metric.addMeasure(new Measure("threads", "Count", storageResolution));
        metric.addMeasure(new Measure("files", "Count", storageResolution));
        metric.addMeasure(new Measure("fds", "Count", storageResolution));
        if (metricService != null) {
            metricService.defineMetric(metric);
        } else {
            MetricEmitter.defineMetric(metric);
        }
    }

    /**
     * Publishes a heartbeat message to indicate the component is alive and functioning.
     * The message includes the current timestamp and component information.
     */
    private void publishHeartbeat()
    {
        JsonObject data = heartbeatMonitor.getStats();
        for (HeartbeatConfiguration.HeartbeatTarget target : configManager.getHeartbeatConfig().getTargets())
        {
            switch (target.getType().toLowerCase())
            {
                case "metric":
                    Map<String, Float> measureValues = new HashMap<>();
                    for (Map.Entry<String, JsonElement> entry : data.entrySet())
                    {
                        for (String measureName : entry.getValue().getAsJsonObject().keySet())
                        {
                            measureValues.put(measureName, entry.getValue().getAsJsonObject().get(measureName).getAsFloat());
                        }
                    }
                    if (metricService != null) {
                        metricService.emitMetricNow("heartbeat", measureValues);
                    } else {
                        MetricEmitter.emitMetricNow("heartbeat", measureValues);
                    }
                    break;

                case "messaging":
                    String topic = configManager.resolveTemplate(HeartbeatConfiguration.DEFAULT_TOPIC);
                    String destination = HeartbeatConfiguration.DEFAULT_MESSAGING_DESTINATION;

                    if (target.getConfig().has("destination"))
                    {
                        destination = target.getConfig().get("destination").getAsString();
                    }
                    if (target.getConfig().has("topic"))
                    {
                        topic = configManager.resolveTemplate(target.getConfig().get("topic").getAsString());
                    }

                    if (destination.equalsIgnoreCase("ipc"))
                    {
                        if (messagingService != null) {
                            messagingService.publish(topic, Message.buildFromConfig(
                                    MESSAGE_NAME, MESSAGE_VERSION, heartbeatMonitor.getStats(), configManager
                            ));
                        } else {
                            MessagingClient.publish(topic, Message.buildFromConfig(
                                    MESSAGE_NAME, MESSAGE_VERSION, heartbeatMonitor.getStats(), configManager
                            ));
                        }
                    }
                    else if (destination.equalsIgnoreCase("iot_core"))
                    {
                        if (messagingService != null) {
                            messagingService.publishToIotCore(topic, Message.buildFromConfig(
                                    MESSAGE_NAME, MESSAGE_VERSION, heartbeatMonitor.getStats(), configManager
                            ), QOS.AT_LEAST_ONCE);
                        } else {
                            MessagingClient.publishToIotCore(topic, Message.buildFromConfig(
                                    MESSAGE_NAME, MESSAGE_VERSION, heartbeatMonitor.getStats(), configManager
                            ), QOS.AT_LEAST_ONCE);
                        }
                    }
                    else
                    {
                        LOGGER.warn("Unrecognized messaging destination: '{}'. Ignoring.", destination);
                    }
                    break;
            }
        }

    }

    @Override
    /**
     * Handles configuration changes by reinitializing the heartbeat mechanism.
     * 
     * @return true if the configuration change was handled successfully
     */
    public boolean onConfigurationChanged()
    {
        LOGGER.info("Configuration changed, restarting heartbeat");
        initHeartbeat();
        return true;
    }

    /**
     * Inner class that implements the periodic heartbeat task.
     * Executes the heartbeat publishing operation at configured intervals.
     */
    private class Heartbeater extends TimerTask
    {
        @Override
        public void run()
        {
            publishHeartbeat();
        }
    }
}
