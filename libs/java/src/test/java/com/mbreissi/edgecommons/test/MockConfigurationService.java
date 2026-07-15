/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.test;

import com.mbreissi.edgecommons.config.ConfigManager;
import com.mbreissi.edgecommons.config.ConfigurationChangeListener;
import com.mbreissi.edgecommons.config.ConfigurationFactory;
import com.mbreissi.edgecommons.config.HealthConfiguration;
import com.mbreissi.edgecommons.config.HeartbeatConfiguration;
import com.mbreissi.edgecommons.config.LoggingConfiguration;
import com.mbreissi.edgecommons.config.MetricConfiguration;
import com.mbreissi.edgecommons.config.TagConfiguration;
import com.mbreissi.edgecommons.messaging.MessageIdentity;
import com.google.gson.JsonObject;
import java.util.ArrayList;
import java.util.Collection;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

/**
 * Mock implementation of IConfigurationService for testing.
 */
public class MockConfigurationService extends ConfigManager {
    private JsonObject fullConfig = new JsonObject();
    private JsonObject globalConfig = new JsonObject();
    private Map<String, JsonObject> instanceConfigs = new HashMap<>();
    private String thingName = "test-thing";
    private String componentName = "TestComponent";
    private String componentFullName = "com.test.TestComponent";
    private List<ConfigurationChangeListener> listeners = new ArrayList<>();
    /**
     * The mock's resolved UNS identity — a real ConfigManager always resolves one, so the default
     * mirrors the zero-config resolution (single 'device' level = thing name, component = short
     * name, component scope — no instance, D‑U28). Settable (incl. to null) for identity-edge-case tests.
     */
    private MessageIdentity componentIdentity = new MessageIdentity(
            List.of(new MessageIdentity.HierEntry("device", thingName)),
            componentName, null);
    
    public void setFullConfig(JsonObject config) {
        this.fullConfig = config;
        if (config.has("component")) {
            JsonObject component = config.getAsJsonObject("component");
            if (component.has("global")) {
                this.globalConfig = component.getAsJsonObject("global");
            }
            if (component.has("instances")) {
                instanceConfigs.clear();
                component.getAsJsonArray("instances").forEach(element -> {
                    JsonObject instance = element.getAsJsonObject();
                    String id = instance.get("id").getAsString();
                    instanceConfigs.put(id, instance);
                });
            }
        }
    }
    
    @Override
    public JsonObject getGlobalConfig() {
        return globalConfig;
    }
    
    @Override
    public JsonObject getInstanceConfig(String instanceId) {
        return instanceConfigs.get(instanceId);
    }
    
    @Override
    public Collection<String> getInstanceIds() {
        return instanceConfigs.keySet();
    }
    
    @Override
    public JsonObject getFullConfig() {
        return fullConfig;
    }
    
    @Override
    public String getThingName() {
        return thingName;
    }
    
    public void setThingName(String thingName) {
        this.thingName = thingName;
    }

    @Override
    public MessageIdentity getComponentIdentity() {
        return componentIdentity;
    }

    /** Injects a custom resolved identity ({@code null} = the unresolved/bring-up case). */
    public void setComponentIdentity(MessageIdentity componentIdentity) {
        this.componentIdentity = componentIdentity;
    }
    
    @Override
    public String getComponentName() {
        return componentName;
    }
    
    public void setComponentName(String componentName) {
        this.componentName = componentName;
    }
    
    @Override
    public String getComponentFullName() {
        return componentFullName;
    }
    
    public void setComponentFullName(String componentFullName) {
        this.componentFullName = componentFullName;
    }
    
    @Override
    public String resolveTemplate(String template) {
        return template
            .replace("{ThingName}", thingName)
            .replace("{ComponentName}", componentName)
            .replace("{ComponentFullName}", componentFullName);
    }
    
    @Override
    public void addConfigChangeListener(ConfigurationChangeListener listener) {
        listeners.add(listener);
    }
    
    @Override
    public void removeConfigChangeListener(ConfigurationChangeListener listener) {
        listeners.remove(listener);
    }
    
    @Override
    public void notifyConfigurationChanged() {
        listeners.forEach(ConfigurationChangeListener::onConfigurationChanged);
    }
    
    public void simulateConfigurationChange() {
        notifyConfigurationChanged();
    }
    
    @Override
    public HeartbeatConfiguration getHeartbeatConfig() {
        return ConfigurationFactory.createHeartbeatConfiguration(null);
    }
    
    @Override
    public TagConfiguration getTagConfig() {
        return ConfigurationFactory.createTagConfiguration(null);
    }
    
    @Override
    public LoggingConfiguration getLoggingConfig() {
        return loggingConfig;
    }

    /** Injects a custom logging configuration. */
    public void setLoggingConfig(LoggingConfiguration loggingConfig) {
        this.loggingConfig = loggingConfig;
    }
    
    @Override
    public MetricConfiguration getMetricConfig() {
        return ConfigurationFactory.createMetricConfiguration(null);
    }

    private HealthConfiguration healthConfig = ConfigurationFactory.createHealthConfiguration(null);
    private LoggingConfiguration loggingConfig = ConfigurationFactory.createLoggingConfiguration(null);

    @Override
    public HealthConfiguration getHealthConfig() {
        return healthConfig;
    }

    /** Injects a custom health configuration (for health-server enablement/port tests). */
    public void setHealthConfig(HealthConfiguration healthConfig) {
        this.healthConfig = healthConfig;
    }
}
