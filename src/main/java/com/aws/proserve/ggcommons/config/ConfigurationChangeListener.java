package com.aws.proserve.ggcommons.config;

public interface ConfigurationChangeListener
{
    // Implementations of onConfigurationChanged() should return true if the configuration was changed.
    boolean onConfigurationChanged();
}
