/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.messaging;

import com.google.gson.Gson;
import org.junit.jupiter.api.Test;

import java.io.File;
import java.nio.file.Files;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;

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

    @Test
    void acceptsKubernetesServiceDnsHost() {
        // FR-MSG-2: a k8s Service DNS name is an opaque host string — no special handling, no insecure
        // behavior. It flows through verbatim as the broker host (the provider builds ssl://host:port
        // / tcp://host:port from it). Mirrors the Rust/TS parity test.
        String json = """
                { "messaging": { "local": {
                  "type":"mqtt","host":"emqx.mqtt.svc.cluster.local","port":1883,"clientId":"c" } } }""";
        MessagingConfiguration cfg = new Gson().fromJson(json, MessagingConfiguration.class);
        assertEquals("emqx.mqtt.svc.cluster.local", cfg.getMessaging().getLocal().getHost());
        assertEquals(1883, cfg.getMessaging().getLocal().getPort());
    }

    @Test
    void singleBrokerTopologyWhenIotCoreAbsent() {
        // FR-MSG-3: no 'iotCore' section => single-broker topology (local only / air-gapped). The
        // provider constructs only the local client and leaves the IoT Core client null.
        String json = """
                { "messaging": { "local": {
                  "type":"mqtt","host":"emqx.mqtt.svc.cluster.local","port":1883,"clientId":"c" } } }""";
        MessagingConfiguration cfg = new Gson().fromJson(json, MessagingConfiguration.class);
        assertNotNull(cfg.getMessaging().getLocal());
        assertNull(cfg.getMessaging().getIotCore(), "absent iotCore => single-broker topology");
    }

    @Test
    void dualBrokerTopologyWhenIotCorePresent() {
        // FR-MSG-3: an 'iotCore' section => dual-MQTT (local broker + AWS IoT Core). The IoT Core leg
        // is mutual-TLS (cert/key/CA) with no insecure fallback (StandaloneMessagingProvider refuses to
        // connect when the socket factory can't be built).
        String json = """
                { "messaging": {
                  "local": {"type":"mqtt","host":"emqx.mqtt.svc.cluster.local","port":1883,"clientId":"l"},
                  "iotCore": {"endpoint":"x.iot.amazonaws.com","port":8883,"clientId":"i",
                    "credentials":{"certPath":"c.pem","keyPath":"k.pem","caPath":"ca.pem"}} } }""";
        MessagingConfiguration cfg = new Gson().fromJson(json, MessagingConfiguration.class);
        assertNotNull(cfg.getMessaging().getIotCore(), "present iotCore => dual-broker topology");
        assertEquals("x.iot.amazonaws.com", cfg.getMessaging().getIotCore().getEndpoint());
        assertEquals(8883, cfg.getMessaging().getIotCore().getPort());
        // mutual-TLS material is present (no username/password / insecure path on the IoT Core leg).
        assertEquals("c.pem", cfg.getMessaging().getIotCore().getCredentials().getCertPath());
        assertEquals("k.pem", cfg.getMessaging().getIotCore().getCredentials().getKeyPath());
        assertEquals("ca.pem", cfg.getMessaging().getIotCore().getCredentials().getCaPath());
    }
}
