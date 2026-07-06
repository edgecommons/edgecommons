/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.messaging;

import com.google.gson.Gson;
import org.junit.jupiter.api.Test;

import java.io.File;
import java.nio.file.Files;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Tests for {@link MessagingConfiguration} deserialization and accessors, covering the local +
 * northbound sections and both credential styles (username/password and cert/key/CA).
 */
class MessagingConfigurationTest {

    private static final String JSON = """
            { "messaging": {\
            "local": {"type":"mqtt","host":"localhost","port":1883,"clientId":"loc",\
              "credentials":{"username":"u","password":"p"}},\
            "northbound": {"endpoint":"x.mqtt.example.com","port":8883,"clientId":"north",\
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

        MessagingConfiguration.NorthboundMqttConfig northbound = m.getNorthbound();
        assertEquals("x.mqtt.example.com", northbound.getEndpoint());
        assertEquals(8883, northbound.getPort());
        assertEquals("north", northbound.getClientId());
        assertEquals("c.pem", northbound.getCredentials().getCertPath());
        assertEquals("k.pem", northbound.getCredentials().getKeyPath());
        assertEquals("ca.pem", northbound.getCredentials().getCaPath());
    }

    @Test
    void loadFromFileReadsConfig() throws Exception {
        File tmp = File.createTempFile("messaging", ".json");
        tmp.deleteOnExit();
        Files.write(tmp.toPath(), JSON.getBytes());

        MessagingConfiguration cfg = MessagingConfiguration.loadFromFile(tmp.getAbsolutePath());
        assertEquals(1883, cfg.getMessaging().getLocal().getPort());
        assertEquals(8883, cfg.getMessaging().getNorthbound().getPort());
    }

    @Test
    void loadFromFileRejectsGenericMessagingLwt() throws Exception {
        File tmp = File.createTempFile("messaging-lwt", ".json");
        tmp.deleteOnExit();
        Files.write(tmp.toPath(), """
                { "messaging": {
                  "local": {"host":"localhost","port":1883,"clientId":"loc"},
                  "lwt": {"topic":"ecv1/d/uns-bridge/main/state"}
                } }""".getBytes());

        IllegalArgumentException err = assertThrows(
                IllegalArgumentException.class,
                () -> MessagingConfiguration.loadFromFile(tmp.getAbsolutePath()));
        assertTrue(err.getMessage().contains("messaging.lwt is not supported"));
    }

    @Test
    void deserializesQosDefaults() {
        String json = """
                { "messaging": {
                  "local": {"type":"mqtt","host":"localhost","port":1883,"clientId":"c",
                            "qos": {"publish":2,"subscribe":0}},
                  "northbound": {"host":"broker.example.com","port":8883,"clientId":"n",
                                 "qos": {"publish":2,"subscribe":0}}
                } }""";
        MessagingConfiguration cfg = new Gson().fromJson(json, MessagingConfiguration.class);
        assertNotNull(cfg.getMessaging().getLocal().getQos());
        assertEquals(2, cfg.getMessaging().getLocal().getQos().publishOrDefault());
        assertEquals(0, cfg.getMessaging().getLocal().getQos().subscribeOrDefault());
        assertNotNull(cfg.getMessaging().getNorthbound().getQos());
        assertEquals(2, cfg.getMessaging().getNorthbound().getQos().publishOrDefault());
        assertEquals(0, cfg.getMessaging().getNorthbound().getQos().subscribeOrDefault());
    }

    @Test
    void loadFromFileRejectsTopLevelMessagingQos() throws Exception {
        File tmp = File.createTempFile("messaging-qos", ".json");
        tmp.deleteOnExit();
        Files.write(tmp.toPath(), """
                { "messaging": {
                  "local": {"host":"localhost","port":1883,"clientId":"loc"},
                  "qos": {"local": {"publish": 1}}
                } }""".getBytes());

        IllegalArgumentException err = assertThrows(
                IllegalArgumentException.class,
                () -> MessagingConfiguration.loadFromFile(tmp.getAbsolutePath()));
        assertTrue(err.getMessage().contains("messaging.qos is not supported"));
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
    void singleBrokerTopologyWhenNorthboundAbsent() {
        // FR-MSG-3: no 'northbound' section => single-broker topology (local only / air-gapped). The
        // provider constructs only the local client and leaves the northbound client null.
        String json = """
                { "messaging": { "local": {
                  "type":"mqtt","host":"emqx.mqtt.svc.cluster.local","port":1883,"clientId":"c" } } }""";
        MessagingConfiguration cfg = new Gson().fromJson(json, MessagingConfiguration.class);
        assertNotNull(cfg.getMessaging().getLocal());
        assertNull(cfg.getMessaging().getNorthbound(), "absent northbound => single-broker topology");
    }

    @Test
    void dualBrokerTopologyWhenNorthboundPresent() {
        // FR-MSG-3: a 'northbound' section => dual-MQTT (local broker + generic northbound broker).
        // TLS is selected when a CA path is present; cert/key may add mutual TLS.
        String json = """
                { "messaging": {
                  "local": {"type":"mqtt","host":"emqx.mqtt.svc.cluster.local","port":1883,"clientId":"l"},
                  "northbound": {"endpoint":"x.mqtt.example.com","port":8883,"clientId":"n",
                    "credentials":{"certPath":"c.pem","keyPath":"k.pem","caPath":"ca.pem"}} } }""";
        MessagingConfiguration cfg = new Gson().fromJson(json, MessagingConfiguration.class);
        assertNotNull(cfg.getMessaging().getNorthbound(), "present northbound => dual-broker topology");
        assertEquals("x.mqtt.example.com", cfg.getMessaging().getNorthbound().getEndpoint());
        assertEquals(8883, cfg.getMessaging().getNorthbound().getPort());
        assertEquals("c.pem", cfg.getMessaging().getNorthbound().getCredentials().getCertPath());
        assertEquals("k.pem", cfg.getMessaging().getNorthbound().getCredentials().getKeyPath());
        assertEquals("ca.pem", cfg.getMessaging().getNorthbound().getCredentials().getCaPath());
    }
}
