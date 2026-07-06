/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.metrics;

/**
 * Represents a measure within a metric.
 * Contains the measure name and any additional properties like unit or aggregation type.
 *
 * <p>An immutable value type (Java record). Accessors are {@code name()}, {@code unit()},
 * and {@code storageResolution()}.
 */
public record Measure(String name, String unit, int storageResolution)
{
    private static final int DEFAULT_STORAGE_RESOLUTION = 60;

    /**
     * Canonical constructor. Clamps the storage resolution to CloudWatch's two
     * supported values: 1 second (high resolution) or 60 seconds (standard).
     */
    public Measure
    {
        storageResolution = storageResolution < 60 ? 1 : 60;
    }

    /**
     * Creates a new measure with name and unit at the standard (60s) storage resolution.
     *
     * @param name The name of the measure
     * @param unit The unit of measurement
     */
    public Measure(String name, String unit)
    {
        this(name, unit, DEFAULT_STORAGE_RESOLUTION);
    }
}
