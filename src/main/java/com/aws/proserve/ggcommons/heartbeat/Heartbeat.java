/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.heartbeat;

import com.aws.proserve.ggcommons.config.ConfigManager;
import com.aws.proserve.ggcommons.config.ConfigurationChangeListener;
import com.aws.proserve.ggcommons.config.HeartbeatConfiguration;
import com.aws.proserve.ggcommons.messaging.Message;
import com.aws.proserve.ggcommons.messaging.MessageBuilder;
import com.aws.proserve.ggcommons.messaging.MessagingClient;
import com.aws.proserve.ggcommons.metrics.Metric;
import com.aws.proserve.ggcommons.metrics.MetricBuilder;
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
    private final ConfigManager configurationService;
    private MessagingClient messagingService;
    private MetricEmitter metricService;
    private HeartbeatMonitor heartbeatMonitor;
    private Timer heartbeatTimer;
    private final Object timerLock = new Object();

    /**
     * Package-private constructor used by HeartbeatBuilder.
     * Use HeartbeatBuilder.create() instead of calling this directly.
     */
    Heartbeat(ConfigManager configurationService, MessagingClient messagingService, MetricEmitter metricService)
    {
        this.configurationService = configurationService;
        this.messagingService = messagingService;
        this.metricService = metricService;
        
        configurationService.addConfigChangeListener(this);
        initialize();
    }
    
    /**
     * Initializes the heartbeat after all dependencies are set.
     */
    private void initialize() {
        defineMetric();
        initHeartbeat();
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
            heartbeatMonitor = new HeartbeatMonitor(configurationService.getHeartbeatConfig());
            heartbeatTimer = new Timer("Heartbeat timer", true);
            heartbeatTimer.scheduleAtFixedRate(new Heartbeater(), 0, configurationService.getHeartbeatConfig().getIntervalSecs()*1000L);
            LOGGER.info("Heartbeat initialized at {} second interval", configurationService.getHeartbeatConfig().getIntervalSecs());
        }
    }

    /**
     * Defines the heartbeat metric in the metrics system.
     * This metric is used to track the component's health status.
     */
    private void defineMetric()
    {
        int storageResolution = configurationService.getHeartbeatConfig().getIntervalSecs() < 60 ? 1 : 60;
        Metric metric = MetricBuilder.create("heartbeat")
                .withNamespace(configurationService.getMetricConfig().getNamespace())
                .withConfig(configurationService)
                .addMeasure("disk_total", "Gigabytes", storageResolution)
                .addMeasure("disk_used", "Gigabytes", storageResolution)
                .addMeasure("disk_free", "Gigabytes", storageResolution)
                .addMeasure("cpu_usage", "Percent", storageResolution)
                .addMeasure("memory_usage", "Megabytes", storageResolution)
                .addMeasure("threads", "Count", storageResolution)
                .addMeasure("files", "Count", storageResolution)
                .addMeasure("fds", "Count", storageResolution)
                .build();
        metricService.defineMetric(metric);
    }

    /**
     * Publishes a heartbeat message to indicate the component is alive and functioning.
     * The message includes the current timestamp and component information.
     */
    private void publishHeartbeat()
    {
        JsonObject data = heartbeatMonitor.getStats();
        for (HeartbeatConfiguration.HeartbeatTarget target : configurationService.getHeartbeatConfig().getTargets())
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
                    metricService.emitMetricNow("heartbeat", measureValues);
                    break;

                case "messaging":
                    String topic = configurationService.resolveTemplate(HeartbeatConfiguration.DEFAULT_TOPIC);
                    String destination = HeartbeatConfiguration.DEFAULT_MESSAGING_DESTINATION;

                    if (target.getConfig().has("destination"))
                    {
                        destination = target.getConfig().get("destination").getAsString();
                    }
                    if (target.getConfig().has("topic"))
                    {
                        topic = configurationService.resolveTemplate(target.getConfig().get("topic").getAsString());
                    }

                    Message heartbeatMessage = MessageBuilder.create(MESSAGE_NAME, MESSAGE_VERSION)
                            .withPayload(heartbeatMonitor.getStats())
                            .withConfig(configurationService)
                            .build();
                    
                    if (destination.equalsIgnoreCase("ipc"))
                    {
                        messagingService.publish(topic, heartbeatMessage);
                    }
                    else if (destination.equalsIgnoreCase("iot_core"))
                    {
                        messagingService.publishToIotCore(topic, heartbeatMessage, QOS.AT_LEAST_ONCE);
                    }
                    else
                    {
                        LOGGER.warn("Unrecognized messaging destination: '{}'. Ignoring.", destination);
                    }
                    break;
            }
        }

    }

    /**
     * Stops the heartbeat, cancelling its periodic timer.
     */
    public void close()
    {
        synchronized (timerLock)
        {
            if (heartbeatTimer != null)
            {
                heartbeatTimer.cancel();
                heartbeatTimer.purge();
                heartbeatTimer = null;
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
