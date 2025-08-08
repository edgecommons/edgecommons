/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.config;

import com.aws.proserve.ggcommons.interfaces.IConfigurationService;
import com.google.gson.JsonObject;
import java.util.Collection;

/**
 * Service implementation that wraps ConfigManager to provide the IConfigurationService interface.
 * This allows for dependency injection while maintaining backward compatibility.
 */
public class ConfigurationService implements IConfigurationService {
    private final ConfigManager configManager;
    
    public ConfigurationService(ConfigManager configManager) {
        this.configManager = configManager;
    }
    
    @Override
    public JsonObject getGlobalConfig() {
        return configManager.getGlobalConfig();
    }
    
    @Override
    public JsonObject getInstanceConfig(String instanceId) {
        return configManager.getInstanceConfig(instanceId);
    }
    
    @Override
    public Collection<String> getInstanceIds() {
        return configManager.getInstanceIds();
    }
    
    @Override
    public JsonObject getFullConfig() {
        return configManager.getFullConfig();
    }
    
    @Override
    public String getThingName() {
        return configManager.getThingName();
    }
    
    @Override
    public String getComponentName() {
        return configManager.getComponentName();
    }
    
    @Override
    public String getComponentFullName() {
        return configManager.getComponentFullName();
    }
    
    @Override
    public String resolveTemplate(String template) {
        return configManager.resolveTemplate(template);
    }
    
    @Override
    public void addConfigChangeListener(ConfigurationChangeListener listener) {
        configManager.addConfigChangeListener(listener);
    }
    
    @Override
    public void removeConfigChangeListener(ConfigurationChangeListener listener) {
        configManager.removeConfigChangeListener(listener);
    }
    
    @Override
    public void notifyConfigurationChanged() {
        configManager.notifyConfigurationChanged();
    }
    
    @Override
    public HeartbeatConfiguration getHeartbeatConfig() {
        return configManager.getHeartbeatConfig();
    }
    
    @Override
    public TagConfiguration getTagConfig() {
        return configManager.getTagConfig();
    }
    
    @Override
    public LoggingConfiguration getLoggingConfig() {
        return configManager.getLoggingConfig();
    }
    
    @Override
    public MetricConfiguration getMetricConfig() {
        return configManager.getMetricConfig();
    }
}