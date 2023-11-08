package com.aws.proserve.ggcommons.config.manager;

public interface ConfigurationChangeListener
{
    // Implementations of onConfigurationChanged() should return true if the configuration was changed.
    boolean onConfigurationChanged();
}
