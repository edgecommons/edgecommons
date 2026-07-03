package com.mbreissi.ggcommons.config;

import com.google.gson.JsonObject;

/**
 * Builder for creating HeartbeatConfiguration instances programmatically
 * (UNS-CANONICAL-DESIGN §4.3 shape: {@code enabled / intervalSecs / measures / destination};
 * the legacy {@code targets[]} array is removed — D-U20).
 */
public class HeartbeatConfigurationBuilder {
    private boolean enabled = true;
    private int intervalSecs = 5;
    private boolean includeCpu = true;
    private boolean includeMemory = true;
    private boolean includeThreads = false;
    private boolean includeFiles = false;
    private String destination = HeartbeatConfiguration.DEFAULT_DESTINATION;

    private HeartbeatConfigurationBuilder() {}

    public static HeartbeatConfigurationBuilder create() {
        return new HeartbeatConfigurationBuilder();
    }

    /** Enables/disables the heartbeat (state keepalive + {@code sys} measures metric). */
    public HeartbeatConfigurationBuilder withEnabled(boolean enabled) {
        this.enabled = enabled;
        return this;
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

    /**
     * Sets the state keepalive's publish destination — {@code "local"} (default) or
     * {@code "iotcore"}.
     */
    public HeartbeatConfigurationBuilder withDestination(String destination) {
        if (!"local".equalsIgnoreCase(destination) && !"iotcore".equalsIgnoreCase(destination)) {
            throw new IllegalArgumentException(
                    "Heartbeat destination must be 'local' or 'iotcore' (got '" + destination + "')");
        }
        this.destination = destination;
        return this;
    }

    public HeartbeatConfiguration build() {
        JsonObject config = new JsonObject();
        config.addProperty("enabled", enabled);
        config.addProperty("intervalSecs", intervalSecs);

        JsonObject measures = new JsonObject();
        measures.addProperty("cpu", includeCpu);
        measures.addProperty("memory", includeMemory);
        measures.addProperty("threads", includeThreads);
        measures.addProperty("files", includeFiles);
        config.add("measures", measures);

        config.addProperty("destination", destination);

        return new HeartbeatConfiguration(config);
    }
}
