/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.messaging;

import com.google.gson.Gson;
import com.google.gson.JsonObject;

import java.io.FileReader;
import java.io.IOException;

/**
 * Configuration class for standalone messaging setup.
 * Handles both local MQTT broker and AWS IoT Core connection settings.
 */
public class MessagingConfiguration {
    private MessagingConfig messaging;

    public static class MessagingConfig {
        private LocalMqttConfig local;
        private IoTCoreConfig iotCore;

        public LocalMqttConfig getLocal() { return local; }
        public IoTCoreConfig getIotCore() { return iotCore; }
    }

    public static class LocalMqttConfig {
        private String type;
        private String host;
        private int port;
        private String clientId;
        private CredentialsConfig credentials;

        public String getType() { return type; }
        public String getHost() { return host; }
        public int getPort() { return port; }
        public String getClientId() { return clientId; }
        public CredentialsConfig getCredentials() { return credentials; }
    }

    public static class IoTCoreConfig {
        private String endpoint;
        private int port;
        private String clientId;
        private CredentialsConfig credentials;

        public String getEndpoint() { return endpoint; }
        public int getPort() { return port; }
        public String getClientId() { return clientId; }
        public CredentialsConfig getCredentials() { return credentials; }
    }

    public static class CredentialsConfig {
        private String username;
        private String password;
        private String certPath;
        private String keyPath;
        private String caPath;

        public String getUsername() { return username; }
        public String getPassword() { return password; }
        public String getCertPath() { return certPath; }
        public String getKeyPath() { return keyPath; }
        public String getCaPath() { return caPath; }
    }

    public MessagingConfig getMessaging() { return messaging; }

    public static MessagingConfiguration loadFromFile(String configPath) throws IOException {
        Gson gson = new Gson();
        try (FileReader reader = new FileReader(configPath)) {
            return gson.fromJson(reader, MessagingConfiguration.class);
        }
    }
}