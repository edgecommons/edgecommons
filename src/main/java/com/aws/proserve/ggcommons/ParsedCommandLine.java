/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons;

import org.apache.commons.cli.CommandLine;

/**
 * Data class that holds parsed command line arguments for Greengrass components.
 * This class stores various types of arguments including configuration, messaging,
 * metrics, and component-specific settings.
 */
public class ParsedCommandLine
{
    public CommandLine commandLine;
    /** Arguments related to component configuration settings */
    public String[] configArgs;
    /** Arguments related to messaging provider and settings */
    public String[] messagingArgs;
    /** AWS IoT thing name associated with this component */
    public String thingName;
}
