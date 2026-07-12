/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons;

import org.apache.commons.cli.Options;
import org.junit.jupiter.api.Test;

import java.time.Duration;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Unit tests for EdgeCommonsBuilder class.
 * Tests the builder pattern methods for creating EdgeCommons instances.
 */
class EdgeCommonsBuilderTest {

    @Test
    void testBuilderWithArgs() {
        String[] args = {"-t", "test-thing", "-c", "FILE", "./config.json"};
        
        EdgeCommonsBuilder builder = EdgeCommonsBuilder.create("test.component")
                .withArgs(args);
        
        assertNotNull(builder);
    }
    
    @Test
    void testBuilderWithAppOptions() {
        Options options = new Options();
        options.addOption("test", true, "Test option");
        options.addOption("verbose", false, "Verbose output");
        
        EdgeCommonsBuilder builder = EdgeCommonsBuilder.create("test.component")
                .withAppOptions(options);
        
        assertNotNull(builder);
    }
    
    @Test
    void testBuilderReceiveOwnMessages() {
        EdgeCommonsBuilder builder = EdgeCommonsBuilder.create("test.component")
                .receiveOwnMessages(true);
        
        assertNotNull(builder);
        
        EdgeCommonsBuilder builder2 = EdgeCommonsBuilder.create("test.component")
                .receiveOwnMessages(false);
        
        assertNotNull(builder2);
    }
    
    @Test
    void testBuilderChaining() {
        String[] args = {"-t", "test-thing"};
        Options options = new Options();
        options.addOption("test", true, "Test option");
        
        EdgeCommonsBuilder builder = EdgeCommonsBuilder.create("test.component")
                .withArgs(args)
                .withAppOptions(options)
                .receiveOwnMessages(true);
        
        assertNotNull(builder);
    }

    @Test
    void lifecycleConfigurationIsFluentAndRejectsAmbiguousRegistration() {
        EdgeCommonsBuilder builder = EdgeCommonsBuilder.create("test.component")
                .initialReady(false)
                .withConfigValidationTimeout(Duration.ofMillis(250))
                .withConfigurationValidator("camera", (candidate, current, phase) ->
                        com.mbreissi.edgecommons.config.ConfigurationCandidateValidator.Result.accept())
                .configureCommands(inbox -> { });

        assertNotNull(builder);
        assertThrows(IllegalArgumentException.class,
                () -> builder.withConfigurationValidator("camera", (candidate, current, phase) ->
                        com.mbreissi.edgecommons.config.ConfigurationCandidateValidator.Result.accept()));
        assertThrows(IllegalArgumentException.class,
                () -> builder.withConfigValidationTimeout(Duration.ZERO));
        assertThrows(IllegalArgumentException.class,
                () -> builder.withConfigurationValidator(" ", (candidate, current, phase) ->
                        com.mbreissi.edgecommons.config.ConfigurationCandidateValidator.Result.accept()));
    }
}
