/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.metrics;

public class Measure
{
    private final String name;
    private final String unit;

    private final static int DEFAULT_STORAGE_RESOLUTION = 60;
    private final int storageResolution;

    public Measure(String name, String unit)
    {
        this(name, unit, DEFAULT_STORAGE_RESOLUTION);
    }

    public Measure(String name, String unit, int storageResolution)
    {
        this.name = name;
        this.unit = unit;
        this.storageResolution = storageResolution < 60 ? 1 : 60;
    }

    public String getName()
    {
        return name;
    }

    public String getUnit()
    {
        return unit;
    }

    public int getStorageResolution()
    {
        return storageResolution;
    }
}
