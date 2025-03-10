/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons;

import org.apache.commons.cli.CommandLine;

public class ParsedCommandLine
{
    public CommandLine commandLine;
    public String[] configArgs;
    public String[] messagingArgs;
    public String thingName;
}
