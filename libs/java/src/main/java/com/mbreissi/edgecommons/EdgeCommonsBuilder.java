package com.mbreissi.edgecommons;

import org.apache.commons.cli.Options;

/**
 * Builder for creating EdgeCommons instances with fluent API.
 */
public class EdgeCommonsBuilder {
    private String componentName;
    private String[] args;
    private Options appOptions;
    private boolean receiveOwnMessages = true;
    
    private EdgeCommonsBuilder() {}
    
    public static EdgeCommonsBuilder create(String componentName) {
        EdgeCommonsBuilder builder = new EdgeCommonsBuilder();
        builder.componentName = componentName;
        return builder;
    }
    
    public EdgeCommonsBuilder withArgs(String[] args) {
        this.args = args;
        return this;
    }
    
    public EdgeCommonsBuilder withAppOptions(Options appOptions) {
        this.appOptions = appOptions;
        return this;
    }
    
    public EdgeCommonsBuilder receiveOwnMessages(boolean receiveOwnMessages) {
        this.receiveOwnMessages = receiveOwnMessages;
        return this;
    }
    
    public EdgeCommons build() {
        if (componentName == null) {
            throw new IllegalStateException("Component name is required");
        }
        if (args == null) {
            args = new String[0];
        }
        
        EdgeCommons edgeCommons = new EdgeCommons();
        edgeCommons.init(componentName, args, appOptions, receiveOwnMessages);
        return edgeCommons;
    }
}