/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.di;

import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;

/**
 * Simple dependency injection container for managing service instances.
 * Provides registration and lookup of services by type.
 */
public class ServiceRegistry {
    private final Map<Class<?>, Object> services = new ConcurrentHashMap<>();
    
    /**
     * Registers a service implementation for the given service type.
     * 
     * @param serviceType The service interface or class type
     * @param implementation The service implementation instance
     * @param <T> The service type
     */
    public <T> void register(Class<T> serviceType, T implementation) {
        if (serviceType == null) {
            throw new IllegalArgumentException("Service type cannot be null");
        }
        if (implementation == null) {
            throw new IllegalArgumentException("Implementation cannot be null");
        }
        services.put(serviceType, implementation);
    }
    
    /**
     * Retrieves a service implementation by type.
     * 
     * @param serviceType The service type to retrieve
     * @param <T> The service type
     * @return The service implementation, or null if not registered
     */
    @SuppressWarnings("unchecked")
    public <T> T get(Class<T> serviceType) {
        Object service = services.get(serviceType);
        return service != null ? (T) service : null;
    }
    
    /**
     * Checks if a service is registered for the given type.
     * 
     * @param serviceType The service type to check
     * @return true if registered, false otherwise
     */
    public boolean isRegistered(Class<?> serviceType) {
        return services.containsKey(serviceType);
    }
    
    /**
     * Removes a service registration.
     * 
     * @param serviceType The service type to unregister
     */
    public void unregister(Class<?> serviceType) {
        services.remove(serviceType);
    }
    
    /**
     * Clears all service registrations.
     */
    public void clear() {
        services.clear();
    }
}