/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.config;

/** The lifecycle phase in which a configuration candidate is being validated. */
public enum ConfigurationValidationPhase {
    /** The first candidate, before the configuration provider starts watching for changes. */
    INITIAL,
    /** Any candidate received after the initial snapshot committed. */
    RELOAD
}
