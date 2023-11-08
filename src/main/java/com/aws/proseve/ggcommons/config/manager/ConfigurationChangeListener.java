package com.aws.proseve.ggcommons.config.manager;

public interface ConfigurationChangeListener
{
    // Implementations of onConfigurationChanged() should return true if the configuration was changed.
    boolean onConfigurationChanged();
}
