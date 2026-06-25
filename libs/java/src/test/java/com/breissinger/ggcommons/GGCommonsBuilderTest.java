/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons;

import org.apache.commons.cli.Options;
import org.junit.jupiter.api.Test;
import static org.junit.jupiter.api.Assertions.*;

/**
 * Unit tests for GGCommonsBuilder class.
 * Tests the builder pattern methods for creating GGCommons instances.
 */
class GGCommonsBuilderTest {

    @Test
    void testBuilderWithArgs() {
        String[] args = {"-t", "test-thing", "-c", "FILE", "./config.json"};
        
        GGCommonsBuilder builder = GGCommonsBuilder.create("test.component")
                .withArgs(args);
        
        assertNotNull(builder);
    }
    
    @Test
    void testBuilderWithAppOptions() {
        Options options = new Options();
        options.addOption("test", true, "Test option");
        options.addOption("verbose", false, "Verbose output");
        
        GGCommonsBuilder builder = GGCommonsBuilder.create("test.component")
                .withAppOptions(options);
        
        assertNotNull(builder);
    }
    
    @Test
    void testBuilderReceiveOwnMessages() {
        GGCommonsBuilder builder = GGCommonsBuilder.create("test.component")
                .receiveOwnMessages(true);
        
        assertNotNull(builder);
        
        GGCommonsBuilder builder2 = GGCommonsBuilder.create("test.component")
                .receiveOwnMessages(false);
        
        assertNotNull(builder2);
    }
    
    @Test
    void testBuilderChaining() {
        String[] args = {"-t", "test-thing"};
        Options options = new Options();
        options.addOption("test", true, "Test option");
        
        GGCommonsBuilder builder = GGCommonsBuilder.create("test.component")
                .withArgs(args)
                .withAppOptions(options)
                .receiveOwnMessages(true);
        
        assertNotNull(builder);
    }
}