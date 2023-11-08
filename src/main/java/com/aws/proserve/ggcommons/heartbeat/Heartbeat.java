package com.aws.proserve.ggcommons.heartbeat;

import com.aws.proserve.ggcommons.config.manager.ConfigManager;
import com.aws.proserve.ggcommons.config.manager.ConfigurationChangeListener;
import com.aws.proserve.ggcommons.messaging.Message;
import com.aws.proserve.ggcommons.messaging.MessagingClient;
import com.github.cliftonlabs.json_simple.JsonObject;

import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.Timer;
import java.util.TimerTask;

public class Heartbeat implements ConfigurationChangeListener
{
    protected static final Logger LOGGER = LogManager.getLogger(Heartbeat.class);
    private final static String MESSAGE_NAME = "heartbeat";
    private final static String MESSAGE_VERSION = "1.0.0";
    private final ConfigManager configManager;
    private String topic;
    private HeartbeatMonitor heartbeatMonitor;
    private Timer heartbeatTimer;

    public Heartbeat(ConfigManager config)
    {
        configManager = config;
        configManager.addConfigChangeListener(this);
        initHeartbeat();
    }

    private void initHeartbeat()
    {
        if (configManager.getHeartbeatConfig() != null)
        {
            topic = configManager.resolveTemplate(configManager.getHeartbeatConfig().getTopic());
            heartbeatMonitor = new HeartbeatMonitor(configManager.getHeartbeatConfig());
            heartbeatTimer = new Timer("Heartbeat timer", true);
            heartbeatTimer.scheduleAtFixedRate(new Heartbeater(), 0, configManager.getHeartbeatConfig().getIntervalSecs()*1000L);
        }
    }

    private void publishHeartbeat()
    {
        JsonObject data = heartbeatMonitor.getStats();
        Message msg = Message.buildFromConfig(MESSAGE_NAME, MESSAGE_VERSION, data, configManager);
        MessagingClient.publish(topic, msg);
    }

    @Override
    public boolean onConfigurationChanged()
    {
        LOGGER.info("Configuration changed, restarting com.aws.proseve.ggcommons.heartbeat");
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
