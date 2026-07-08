/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons;

import com.mbreissi.edgecommons.platform.Platform;
import com.mbreissi.edgecommons.platform.Transport;
import org.apache.commons.cli.CommandLine;

/**
 * Data class that holds parsed command line arguments for Greengrass components, after the
 * platform/transport resolver has run (DESIGN-core §4). It carries the two resolved runtime axes
 * ({@link #platform}, {@link #transport}) plus the resolved config source, messaging-config path and
 * thing name.
 */
public class ParsedCommandLine
{
    public CommandLine commandLine;
    /** Arguments related to component configuration settings (resolved: explicit, else profile default) */
    public String[] configArgs;
    /** Resolved deployment platform (the primary runtime axis). */
    public Platform platform;
    /** Resolved messaging transport (the secondary runtime axis; derived from the platform unless overridden). */
    public Transport transport;
    /** Path to the MQTT messaging-config file (the {@code --transport MQTT <path>} payload). */
    public String standaloneConfigPath;
    /** AWS IoT thing name associated with this component */
    public String thingName;
    /** Parse-time opt-out: disables shared-layer resolution for this process. */
    public boolean noSharedConfig;
}
