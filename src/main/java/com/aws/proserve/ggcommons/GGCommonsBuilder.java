package com.aws.proserve.ggcommons;

import org.apache.commons.cli.Options;

/**
 * Builder for creating GGCommons instances with fluent API.
 */
public class GGCommonsBuilder {
    private String componentName;
    private String[] args;
    private Options appOptions;
    private boolean receiveOwnMessages = true;
    
    private GGCommonsBuilder() {}
    
    public static GGCommonsBuilder create(String componentName) {
        GGCommonsBuilder builder = new GGCommonsBuilder();
        builder.componentName = componentName;
        return builder;
    }
    
    public GGCommonsBuilder withArgs(String[] args) {
        this.args = args;
        return this;
    }
    
    public GGCommonsBuilder withAppOptions(Options appOptions) {
        this.appOptions = appOptions;
        return this;
    }
    
    public GGCommonsBuilder receiveOwnMessages(boolean receiveOwnMessages) {
        this.receiveOwnMessages = receiveOwnMessages;
        return this;
    }
    
    public GGCommons build() {
        if (componentName == null) {
            throw new IllegalStateException("Component name is required");
        }
        if (args == null) {
            args = new String[0];
        }
        
        return new GGCommons(componentName, args, appOptions, receiveOwnMessages);
    }
}