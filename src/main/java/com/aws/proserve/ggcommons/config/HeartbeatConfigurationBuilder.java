package com.aws.proserve.ggcommons.config;

import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import java.util.ArrayList;
import java.util.List;

/**
 * Builder for creating HeartbeatConfiguration instances programmatically.
 */
public class HeartbeatConfigurationBuilder {
    private int intervalSecs = 5;
    private boolean includeCpu = true;
    private boolean includeMemory = true;
    private boolean includeDisk = false;
    private boolean includeThreads = false;
    private boolean includeFiles = false;
    private boolean includeFds = false;
    private List<TargetConfig> targets = new ArrayList<>();
    
    private static class TargetConfig {
        String type;
        JsonObject config;
        
        TargetConfig(String type, JsonObject config) {
            this.type = type;
            this.config = config;
        }
    }
    
    private HeartbeatConfigurationBuilder() {}
    
    public static HeartbeatConfigurationBuilder create() {
        return new HeartbeatConfigurationBuilder();
    }
    
    public HeartbeatConfigurationBuilder withInterval(int intervalSecs) {
        if (intervalSecs < 1) {
            throw new IllegalArgumentException("Interval must be at least 1 second");
        }
        this.intervalSecs = intervalSecs;
        return this;
    }
    
    public HeartbeatConfigurationBuilder includeCpu(boolean include) {
        this.includeCpu = include;
        return this;
    }
    
    public HeartbeatConfigurationBuilder includeMemory(boolean include) {
        this.includeMemory = include;
        return this;
    }
    
    public HeartbeatConfigurationBuilder includeThreads(boolean include) {
        this.includeThreads = include;
        return this;
    }
    
    public HeartbeatConfigurationBuilder includeFiles(boolean include) {
        this.includeFiles = include;
        return this;
    }
    
    public HeartbeatConfigurationBuilder addMetricTarget() {
        targets.add(new TargetConfig("metric", null));
        return this;
    }
    
    public HeartbeatConfigurationBuilder addMessagingTarget(String topic, String destination) {
        JsonObject config = new JsonObject();
        config.addProperty("topic", topic);
        config.addProperty("destination", destination);
        targets.add(new TargetConfig("messaging", config));
        return this;
    }
    
    public HeartbeatConfiguration build() {
        JsonObject config = new JsonObject();
        config.addProperty("intervalSecs", intervalSecs);
        
        JsonObject measures = new JsonObject();
        measures.addProperty("cpu", includeCpu);
        measures.addProperty("memory", includeMemory);
        measures.addProperty("disk", includeDisk);
        measures.addProperty("threads", includeThreads);
        measures.addProperty("files", includeFiles);
        measures.addProperty("fds", includeFds);
        config.add("measures", measures);
        
        if (!targets.isEmpty()) {
            JsonArray targetArray = new JsonArray();
            for (TargetConfig target : targets) {
                JsonObject targetObj = new JsonObject();
                targetObj.addProperty("type", target.type);
                if (target.config != null) {
                    targetObj.add("config", target.config);
                }
                targetArray.add(targetObj);
            }
            config.add("targets", targetArray);
        }
        
        return new HeartbeatConfiguration(config);
    }
}