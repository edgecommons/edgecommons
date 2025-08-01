package com.aws.proserve.ggcommons.di;

import com.aws.proserve.ggcommons.config.ConfigManager;
import com.aws.proserve.ggcommons.interfaces.IConfigurationService;
import com.aws.proserve.ggcommons.interfaces.IMessagingService;
import com.aws.proserve.ggcommons.interfaces.IMetricService;
import com.aws.proserve.ggcommons.messaging.MessagingService;
import com.aws.proserve.ggcommons.metrics.MetricService;

/**
 * Factory for creating service implementations.
 */
public class ServiceFactory {
    
    public static IConfigurationService createConfigurationService(ConfigManager configManager) {
        return configManager;
    }
    
    public static IMessagingService createMessagingService() {
        return new MessagingService();
    }
    
    public static IMetricService createMetricService() {
        return new MetricService();
    }
    
    /**
     * Registers all default services with the provided registry.
     */
    public static void registerDefaultServices(ServiceRegistry registry, ConfigManager configManager) {
        registry.register(IConfigurationService.class, createConfigurationService(configManager));
        registry.register(IMessagingService.class, createMessagingService());
        registry.register(IMetricService.class, createMetricService());
    }
}