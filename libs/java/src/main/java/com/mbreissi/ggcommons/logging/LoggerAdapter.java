/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.logging;

/**
 * Adapter interface for different logging framework implementations.
 */
public interface LoggerAdapter {
    Logger getLogger(String name);
}