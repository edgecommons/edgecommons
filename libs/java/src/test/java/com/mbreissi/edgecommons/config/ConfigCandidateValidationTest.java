/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.config;

import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import org.junit.jupiter.api.Test;

import java.time.Duration;
import java.util.List;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicReference;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

/** Adversarial tests for the pre-commit, generation-atomic configuration lifecycle. */
class ConfigCandidateValidationTest {
    private static JsonObject config(int value) {
        return JsonParser.parseString("""
                {"component":{"global":{"v":%d,"password":"top-secret"}}}
                """.formatted(value)).getAsJsonObject();
    }

    private static ConfigManager manager(ConfigurationCandidateValidator validator,
                                         Duration timeout) {
        return new ConfigManager("com.test.TestComponent", "TestComponent", "thing-1",
                null, config(1), null, null,
                List.of(new CandidateValidationRunner.NamedValidator("camera", validator)),
                timeout);
    }

    @Test
    void initialPhaseHasNoPriorSnapshotAndRejectPreventsConstruction() {
        AtomicReference<ConfigurationValidationPhase> phase = new AtomicReference<>();
        AtomicReference<JsonObject> prior = new AtomicReference<>();
        ConfigurationCandidateValidator validator = (candidate, current, candidatePhase) -> {
            phase.set(candidatePhase);
            prior.set(current);
            return ConfigurationCandidateValidator.Result.reject("CAMERA_UNREACHABLE", "offline");
        };

        IllegalStateException error = assertThrows(IllegalStateException.class,
                () -> manager(validator, Duration.ofSeconds(1)));

        assertEquals(ConfigurationValidationPhase.INITIAL, phase.get());
        assertNull(prior.get());
        assertTrue(error.getMessage().contains("CAMERA_UNREACHABLE"));
    }

    @Test
    void reloadValidatorGetsRedactedPriorAndRejectedGenerationIsInvisible() {
        AtomicReference<JsonObject> priorSeen = new AtomicReference<>();
        AtomicInteger calls = new AtomicInteger();
        ConfigManager manager = manager((candidate, current, phase) -> {
            calls.incrementAndGet();
            if (phase == ConfigurationValidationPhase.INITIAL) {
                return ConfigurationCandidateValidator.Result.accept();
            }
            priorSeen.set(current);
            return ConfigurationCandidateValidator.Result.reject("ENDPOINT_BUSY", "in use");
        }, Duration.ofSeconds(1));
        manager.completeInitialization();
        AtomicBoolean listenerCalled = new AtomicBoolean();
        manager.addConfigChangeListener(() -> {
            listenerCalled.set(true);
            return true;
        });
        JsonObject exactPrior = manager.getFullConfig();

        assertFalse(manager.tryApplyConfig(config(2)));

        assertEquals(2, calls.get());
        assertEquals("***", priorSeen.get()
                .getAsJsonObject("component").getAsJsonObject("global")
                .get("password").getAsString());
        assertEquals(1, manager.getConfigGeneration());
        assertEquals(exactPrior, manager.getFullConfig());
        assertFalse(listenerCalled.get());
        assertEquals("ENDPOINT_BUSY", manager.getLastCandidateValidationErrors().get(0).code());
    }

    @Test
    void timeoutAndFailureRetainExactPriorWithSanitizedStableDiagnostics() {
        AtomicBoolean reload = new AtomicBoolean();
        ConfigManager timeoutManager = manager((candidate, current, phase) -> {
            if (phase == ConfigurationValidationPhase.INITIAL) {
                return ConfigurationCandidateValidator.Result.accept();
            }
            reload.set(true);
            new CountDownLatch(1).await();
            return ConfigurationCandidateValidator.Result.accept();
        }, Duration.ofMillis(30));
        JsonObject timeoutPrior = timeoutManager.getFullConfig();

        assertFalse(timeoutManager.tryApplyConfig(config(2)));
        assertTrue(reload.get());
        assertEquals(timeoutPrior, timeoutManager.getFullConfig());
        assertEquals("VALIDATION_TIMEOUT",
                timeoutManager.getLastCandidateValidationErrors().get(0).code());

        ConfigManager failedManager = manager((candidate, current, phase) -> {
            if (phase == ConfigurationValidationPhase.RELOAD) {
                throw new IllegalStateException("device\nleaked\tcontrol");
            }
            return ConfigurationCandidateValidator.Result.accept();
        }, Duration.ofSeconds(1));

        assertFalse(failedManager.tryApplyConfig(config(2)));
        ConfigurationValidationError failure = failedManager.getLastCandidateValidationErrors().get(0);
        assertEquals("VALIDATOR_FAILED", failure.code());
        assertFalse(failure.message().contains("\n"));
        assertFalse(failure.message().contains("\t"));
        assertEquals(1, failedManager.getConfigGeneration());
    }

    @Test
    void callbackCannotMutateCandidateAndCommitBecomesVisibleOnlyAfterVetoCompletes()
            throws Exception {
        CountDownLatch validatorEntered = new CountDownLatch(1);
        CountDownLatch release = new CountDownLatch(1);
        ConfigManager manager = manager((candidate, current, phase) -> {
            if (phase == ConfigurationValidationPhase.INITIAL) {
                return ConfigurationCandidateValidator.Result.accept();
            }
            candidate.getAsJsonObject("component").getAsJsonObject("global")
                    .addProperty("v", 999);
            validatorEntered.countDown();
            release.await();
            return ConfigurationCandidateValidator.Result.accept();
        }, Duration.ofSeconds(2));

        AtomicBoolean committed = new AtomicBoolean();
        Thread applier = Thread.startVirtualThread(
                () -> committed.set(manager.tryApplyConfig(config(2))));
        assertTrue(validatorEntered.await(1, TimeUnit.SECONDS));

        assertEquals(1, manager.getConfigGeneration());
        assertEquals(1, manager.getGlobalConfig().get("v").getAsInt(),
                "a candidate must not leak before the pre-commit verdict");
        release.countDown();
        applier.join(Duration.ofSeconds(2));

        assertTrue(committed.get());
        assertEquals(2, manager.getConfigGeneration());
        assertEquals(2, manager.getGlobalConfig().get("v").getAsInt(),
                "validator mutation must affect only its defensive copy");
        assertTrue(manager.getLastCandidateValidationErrors().isEmpty());
    }

    @Test
    void aValidatorCannotDriveANestedConfigurationUpdate() {
        // A validator is contractually side-effect free. If one nonetheless calls back into the
        // manager, the update must be refused outright — the outer apply holds the commit lock, so
        // without this guard the validator would deadlock the configuration lifecycle.
        AtomicReference<ConfigManager> self = new AtomicReference<>();
        AtomicReference<Boolean> nestedResult = new AtomicReference<>();
        ConfigManager manager = manager((candidate, current, phase) -> {
            if (phase == ConfigurationValidationPhase.RELOAD) {
                nestedResult.set(self.get().tryApplyConfig(config(99)));
            }
            return ConfigurationCandidateValidator.Result.accept();
        }, Duration.ofSeconds(2));
        self.set(manager);
        manager.completeInitialization();

        assertTrue(manager.tryApplyConfig(config(2)), "the outer candidate still commits");

        assertEquals(Boolean.FALSE, nestedResult.get(),
                "a nested update attempted from inside a validator must be rejected");
        assertEquals(2, manager.getConfigGeneration(),
                "only the outer candidate produced a generation");
        assertEquals(2, manager.getGlobalConfig().get("v").getAsInt());
    }

    @Test
    void aNullCandidateIsRejectedAsAValidationFailureNotAnEscapingThrowable() {
        ConfigManager manager = manager((candidate, current, phase) ->
                ConfigurationCandidateValidator.Result.accept(), Duration.ofSeconds(1));
        manager.completeInitialization();

        assertFalse(manager.tryApplyConfig(null), "a null candidate never commits");

        assertEquals(1, manager.getConfigGeneration(), "the prior generation survives");
        assertEquals("CONFIG_VALIDATION_FAILED",
                manager.getLastCandidateValidationErrors().get(0).code());
        assertEquals("schema", manager.getLastCandidateValidationErrors().get(0).validator());
    }

    @Test
    void theCandidateValidationDeadlineMustBePositiveAndBounded() {
        ConfigurationCandidateValidator accepts = (candidate, current, phase) ->
                ConfigurationCandidateValidator.Result.accept();

        // An unbounded (or absent) deadline would let one hung validator hang the component.
        assertThrows(IllegalArgumentException.class, () -> manager(accepts, null));
        assertThrows(IllegalArgumentException.class, () -> manager(accepts, Duration.ZERO));
        assertThrows(IllegalArgumentException.class,
                () -> manager(accepts, Duration.ofSeconds(-1)));
        assertThrows(IllegalArgumentException.class, () -> manager(accepts,
                ConfigManager.MAX_CANDIDATE_VALIDATION_TIMEOUT.plusSeconds(1)));

        // The bound itself is allowed.
        assertEquals(1, manager(accepts, ConfigManager.MAX_CANDIDATE_VALIDATION_TIMEOUT)
                .getConfigGeneration());
    }

    @Test
    void appliedListenerCannotReplaceGenerationWhileOtherListenersObserveIt() {
        ConfigManager manager = manager((candidate, current, phase) ->
                ConfigurationCandidateValidator.Result.accept(), Duration.ofSeconds(1));
        manager.completeInitialization();
        AtomicBoolean nestedResult = new AtomicBoolean(true);
        AtomicInteger observed = new AtomicInteger();
        manager.addConfigChangeListener(() -> {
            nestedResult.set(manager.tryApplyConfig(config(3)));
            return true;
        });
        manager.addConfigChangeListener(() -> {
            observed.set(manager.getGlobalConfig().get("v").getAsInt());
            return true;
        });

        assertTrue(manager.tryApplyConfig(config(2)));

        assertFalse(nestedResult.get());
        assertEquals(2, observed.get());
        assertEquals(2, manager.getConfigGeneration());
    }
}
