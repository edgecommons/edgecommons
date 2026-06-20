/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.config;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.google.gson.JsonObject;
import com.networknt.schema.JsonSchema;
import com.networknt.schema.JsonSchemaFactory;
import com.networknt.schema.SpecVersion;
import com.networknt.schema.ValidationMessage;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.io.InputStream;
import java.util.Set;

/**
 * Validates GGCommons configuration against JSON schema.
 */
public class ConfigurationValidator {
    private static final Logger LOGGER = LogManager.getLogger(ConfigurationValidator.class);
    private static final String SCHEMA_RESOURCE = "/ggcommons-config-schema.json";
    private static JsonSchema schema;
    
    static {
        try {
            InputStream schemaStream = ConfigurationValidator.class.getResourceAsStream(SCHEMA_RESOURCE);
            if (schemaStream == null) {
                LOGGER.warn("Configuration schema not found at {}, validation disabled", SCHEMA_RESOURCE);
            } else {
                JsonSchemaFactory factory = JsonSchemaFactory.getInstance(SpecVersion.VersionFlag.V7);
                schema = factory.getSchema(schemaStream);
                LOGGER.debug("Configuration schema loaded successfully");
            }
        } catch (Exception e) {
            LOGGER.warn("Failed to load configuration schema: {}, validation disabled", e.getMessage());
        }
    }
    
    /**
     * Validates configuration against the JSON schema.
     * 
     * @param config Configuration to validate
     * @throws ConfigurationValidationException if validation fails
     */
    public static void validate(JsonObject config) throws ConfigurationValidationException {
        if (schema == null) {
            LOGGER.debug("Schema validation skipped - schema not available");
            return;
        }
        
        try {
            ObjectMapper mapper = new ObjectMapper();
            JsonNode configNode = mapper.readTree(config.toString());
            
            Set<ValidationMessage> errors = schema.validate(configNode);
            
            if (!errors.isEmpty()) {
                StringBuilder errorMsg = new StringBuilder("Configuration validation failed:");
                for (ValidationMessage error : errors) {
                    errorMsg.append("\n  - ").append(error.getMessage());
                }
                throw new ConfigurationValidationException(errorMsg.toString());
            }
            
            LOGGER.debug("Configuration validation passed");
            
        } catch (ConfigurationValidationException e) {
            throw e;
        } catch (Exception e) {
            throw new ConfigurationValidationException("Configuration validation error: " + e.getMessage(), e);
        }
    }
    
    /**
     * Exception thrown when configuration validation fails.
     */
    public static class ConfigurationValidationException extends Exception {
        public ConfigurationValidationException(String message) {
            super(message);
        }
        
        public ConfigurationValidationException(String message, Throwable cause) {
            super(message, cause);
        }
    }
}