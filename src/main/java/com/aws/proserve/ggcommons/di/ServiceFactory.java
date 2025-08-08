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
    
    public static IMessagingService createMessagingService(com.aws.proserve.ggcommons.messaging.MessagingClient messagingClient) {
        return new MessagingService(messagingClient);
    }
    
    public static IMetricService createMetricService(com.aws.proserve.ggcommons.metrics.MetricEmitter metricEmitter) {
        return new com.aws.proserve.ggcommons.metrics.MetricService(metricEmitter);
    }
    
    /**
     * Registers default services with the provided registry (excluding messaging and metric services).
     */
    public static void registerDefaultServices(ServiceRegistry registry, ConfigManager configManager) {
        registry.register(IConfigurationService.class, createConfigurationService(configManager));
    }
}