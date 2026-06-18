/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.messaging;

import com.google.gson.Gson;
import org.junit.jupiter.api.Test;

import java.io.File;
import java.nio.file.Files;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;

/**
 * Tests for {@link MessagingConfiguration} deserialization and accessors, covering the local +
 * IoT Core sections and both credential styles (username/password and cert/key/CA).
 */
class MessagingConfigurationTest {

    private static final String JSON = """
            { "messaging": {\
            "local": {"type":"mqtt","host":"localhost","port":1883,"clientId":"loc",\
              "credentials":{"username":"u","password":"p"}},\
            "iotCore": {"endpoint":"x.iot.amazonaws.com","port":8883,"clientId":"iot",\
              "credentials":{"certPath":"c.pem","keyPath":"k.pem","caPath":"ca.pem"}} } }""";

    @Test
    void deserializesAndExposesGetters() {
        MessagingConfiguration cfg = new Gson().fromJson(JSON, MessagingConfiguration.class);
        MessagingConfiguration.MessagingConfig m = cfg.getMessaging();
        assertNotNull(m);

        MessagingConfiguration.LocalMqttConfig local = m.getLocal();
        assertEquals("mqtt", local.getType());
        assertEquals("localhost", local.getHost());
        assertEquals(1883, local.getPort());
        assertEquals("loc", local.getClientId());
        assertEquals("u", local.getCredentials().getUsername());
        assertEquals("p", local.getCredentials().getPassword());

        MessagingConfiguration.IoTCoreConfig iot = m.getIotCore();
        assertEquals("x.iot.amazonaws.com", iot.getEndpoint());
        assertEquals(8883, iot.getPort());
        assertEquals("iot", iot.getClientId());
        assertEquals("c.pem", iot.getCredentials().getCertPath());
        assertEquals("k.pem", iot.getCredentials().getKeyPath());
        assertEquals("ca.pem", iot.getCredentials().getCaPath());
    }

    @Test
    void loadFromFileReadsConfig() throws Exception {
        File tmp = File.createTempFile("messaging", ".json");
        tmp.deleteOnExit();
        Files.write(tmp.toPath(), JSON.getBytes());

        MessagingConfiguration cfg = MessagingConfiguration.loadFromFile(tmp.getAbsolutePath());
        assertEquals(1883, cfg.getMessaging().getLocal().getPort());
        assertEquals(8883, cfg.getMessaging().getIotCore().getPort());
    }
}
