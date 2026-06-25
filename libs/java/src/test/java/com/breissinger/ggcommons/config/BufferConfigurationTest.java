/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons.config;

import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.*;

/** Tests for parsing the cloudwatch {@code buffer} block + the durable default. */
class BufferConfigurationTest {

    private static MetricConfiguration metricConfig(String metricJson) {
        JsonObject obj = JsonParser.parseString(metricJson).getAsJsonObject();
        return new MetricConfiguration(obj);
    }

    @Test
    void cloudwatchDefaultsToDurableWhenNoBufferBlock() {
        BufferConfiguration b = metricConfig(
                "{\"target\":\"cloudwatch\",\"targetConfig\":{\"intervalSecs\":60}}").getBufferConfig();
        assertTrue(b.isDurable());
        assertEquals("durable", b.getType());
        assertEquals(BufferConfiguration.DEFAULT_MAX_DISK_BYTES, b.getMaxDiskBytes());
        assertEquals("dropOldest", b.getOnFull());
        assertEquals("perBatch", b.getFsync());
        assertNotNull(b.getPath());
    }

    @Test
    void explicitMemoryBuffer() {
        BufferConfiguration b = metricConfig(
                "{\"target\":\"cloudwatch\",\"targetConfig\":{\"buffer\":{\"type\":\"memory\"}}}")
                .getBufferConfig();
        assertFalse(b.isDurable());
        assertEquals("memory", b.getType());
    }

    @Test
    void durableBufferFieldsParsed() {
        BufferConfiguration b = metricConfig(
                "{\"target\":\"cloudwatch\",\"targetConfig\":{\"buffer\":{"
                        + "\"type\":\"durable\",\"path\":\"/data/{ComponentName}/cw\","
                        + "\"maxDiskBytes\":1048576,\"onFull\":\"block\",\"fsync\":\"always\"}}}")
                .getBufferConfig();
        assertTrue(b.isDurable());
        assertEquals("/data/{ComponentName}/cw", b.getPath());
        assertEquals(1_048_576L, b.getMaxDiskBytes());
        assertEquals("block", b.getOnFull());
        assertEquals("always", b.getFsync());
    }

    @Test
    void nonPositiveMaxDiskFallsBackToDefault() {
        BufferConfiguration b = metricConfig(
                "{\"target\":\"cloudwatch\",\"targetConfig\":{\"buffer\":{\"maxDiskBytes\":0}}}")
                .getBufferConfig();
        assertEquals(BufferConfiguration.DEFAULT_MAX_DISK_BYTES, b.getMaxDiskBytes());
    }

    @Test
    void floatMaxDiskBytesParsed() {
        // Greengrass delivers numbers as doubles (e.g. 1048576.0).
        BufferConfiguration b = metricConfig(
                "{\"target\":\"cloudwatch\",\"targetConfig\":{\"buffer\":{\"maxDiskBytes\":1048576.0}}}")
                .getBufferConfig();
        assertEquals(1_048_576L, b.getMaxDiskBytes());
    }

    @Test
    void nonCloudwatchTargetIsMemory() {
        BufferConfiguration b = metricConfig("{\"target\":\"log\"}").getBufferConfig();
        assertFalse(b.isDurable());
        assertEquals("memory", b.getType());
    }

    @Test
    void memoryFactoryHasDefaults() {
        BufferConfiguration b = BufferConfiguration.memory();
        assertFalse(b.isDurable());
        assertNull(b.getPath());
        assertEquals(BufferConfiguration.DEFAULT_MAX_DISK_BYTES, b.getMaxDiskBytes());
    }
}
