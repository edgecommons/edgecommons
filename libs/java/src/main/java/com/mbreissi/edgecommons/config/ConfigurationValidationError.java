/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.config;

/** A stable, operator-safe pre-commit validator failure. */
public record ConfigurationValidationError(String validator, String code, String message) { }
