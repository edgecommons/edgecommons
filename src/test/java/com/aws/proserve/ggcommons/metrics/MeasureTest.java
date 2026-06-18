/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.metrics;

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Unit tests for {@link Measure} covering its constructors, getters and the
 * storage-resolution normalization rule.
 */
class MeasureTest {

    @Test
    void twoArgConstructorUsesDefaultStorageResolution() {
        Measure measure = new Measure("Latency", "Milliseconds");

        assertEquals("Latency", measure.getName());
        assertEquals("Milliseconds", measure.getUnit());
        // Default storage resolution is 60 (standard resolution).
        assertEquals(60, measure.getStorageResolution());
    }

    @Test
    void storageResolutionBelow60NormalizesToHighResolution1() {
        Measure measure = new Measure("Cpu", "Percent", 1);
        assertEquals(1, measure.getStorageResolution());
    }

    @Test
    void storageResolutionBelow60AtBoundary59NormalizesTo1() {
        Measure measure = new Measure("Cpu", "Percent", 59);
        assertEquals(1, measure.getStorageResolution());
    }

    @Test
    void storageResolution60StaysStandard() {
        Measure measure = new Measure("Cpu", "Percent", 60);
        assertEquals(60, measure.getStorageResolution());
    }

    @Test
    void storageResolutionAbove60NormalizesToStandard60() {
        Measure measure = new Measure("Cpu", "Percent", 300);
        assertEquals(60, measure.getStorageResolution());
    }

    @Test
    void gettersReturnConstructedValues() {
        Measure measure = new Measure("Throughput", "Count/Second", 1);
        assertEquals("Throughput", measure.getName());
        assertEquals("Count/Second", measure.getUnit());
        assertEquals(1, measure.getStorageResolution());
    }
}
