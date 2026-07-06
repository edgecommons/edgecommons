package com.mbreissi.edgecommons.config;

import com.google.gson.JsonObject;

/**
 * Factory for creating configuration objects from JSON configuration.
 */
public class ConfigurationFactory {
    
    public static TagConfiguration createTagConfiguration(JsonObject config) {
        return config != null && config.has("tags") 
            ? new TagConfiguration(config.get("tags").getAsJsonObject())
            : new TagConfiguration(null);
    }
    
    public static LoggingConfiguration createLoggingConfiguration(JsonObject config) {
        return config != null && config.has("logging")
            ? new LoggingConfiguration(config.get("logging").getAsJsonObject())
            : new LoggingConfiguration(null);
    }
    
    public static HeartbeatConfiguration createHeartbeatConfiguration(JsonObject config) {
        return config != null && config.has("heartbeat")
            ? new HeartbeatConfiguration(config.get("heartbeat").getAsJsonObject())
            : new HeartbeatConfiguration(null);
    }
    
    public static MetricConfiguration createMetricConfiguration(JsonObject config) {
        return config != null && config.has("metricEmission")
            ? new MetricConfiguration(config.get("metricEmission").getAsJsonObject())
            : new MetricConfiguration(null);
    }

    public static HealthConfiguration createHealthConfiguration(JsonObject config) {
        return config != null && config.has("health")
            ? new HealthConfiguration(config.get("health").getAsJsonObject())
            : new HealthConfiguration(null);
    }
}