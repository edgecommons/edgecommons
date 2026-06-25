/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons.config;

public interface ConfigurationChangeListener
{
    // Implementations of onConfigurationChanged() should return true if the configuration was changed.
    boolean onConfigurationChanged();
}
