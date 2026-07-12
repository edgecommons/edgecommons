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
import java.util.concurrent.atomic.AtomicReference;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Contract tests for the bounded runner behind {@code ConfigurationCandidateValidator}
 * (the pre-commit candidate-validation pipeline).
 *
 * <p>The runner is the component's only protection against a misbehaving validator taking the
 * configuration lifecycle down with it. Everything it can be handed — a validator that hangs, one
 * that returns nothing, one that throws an {@link Error} rather than an exception, an interrupted
 * apply, a diagnostic carrying control characters or unbounded operator-hostile text — must come
 * back as a <em>stable, coded, bounded</em> {@link ConfigurationValidationError} and never as an
 * escaping throwable. These tests pin exactly that:
 *
 * <ul>
 *   <li>every validator gets a verdict, and every verdict is either {@code null} (accepted) or a
 *       coded error naming its validator;</li>
 *   <li>the deadline is a hard bound: whatever has not finished when it lapses is
 *       {@code VALIDATION_TIMEOUT}, while verdicts that <em>did</em> land before it are still
 *       collected;</li>
 *   <li>diagnostics are sanitized (control characters stripped, capped at 256 characters);</li>
 *   <li>{@code inValidatorCallback()} is true only on a validator's own thread — the flag that lets
 *       {@code ConfigManager} refuse a nested, side-effecting configuration update.</li>
 * </ul>
 */
class CandidateValidationRunnerTest {

    private static final JsonObject CANDIDATE =
            JsonParser.parseString("{\"component\":{\"global\":{\"v\":1}}}").getAsJsonObject();

    private static CandidateValidationRunner.NamedValidator named(
            String name, ConfigurationCandidateValidator validator) {
        return new CandidateValidationRunner.NamedValidator(name, validator);
    }

    private static List<ConfigurationValidationError> run(
            List<CandidateValidationRunner.NamedValidator> validators, Duration timeout) {
        return CandidateValidationRunner.validate(validators, CANDIDATE, null,
                ConfigurationValidationPhase.RELOAD, timeout);
    }

    /** A validator that never returns. */
    private static ConfigurationCandidateValidator blocksForever() {
        return (candidate, current, phase) -> {
            new CountDownLatch(1).await();
            return ConfigurationCandidateValidator.Result.accept();
        };
    }

    // ===================== verdict collection =====================

    @Test
    void noValidatorsMeansNoWorkAndNoErrors() {
        // A component that registered no validators must not pay for the pipeline at all: no
        // executor, no threads, and an accept verdict even with an already-lapsed deadline.
        assertTrue(run(List.of(), Duration.ofSeconds(1)).isEmpty());
        assertTrue(run(List.of(), Duration.ofNanos(1)).isEmpty());
    }

    @Test
    void everyValidatorIsRunAndOnlyRejectionsAreReported() {
        AtomicBoolean secondRan = new AtomicBoolean();
        List<ConfigurationValidationError> errors = run(List.of(
                named("accepts", (candidate, current, phase) ->
                        ConfigurationCandidateValidator.Result.accept()),
                named("also-accepts", (candidate, current, phase) -> {
                    secondRan.set(true);
                    return ConfigurationCandidateValidator.Result.accept();
                }),
                named("camera", (candidate, current, phase) ->
                        ConfigurationCandidateValidator.Result.reject(
                                "CAMERA_UNREACHABLE", "device offline"))),
                Duration.ofSeconds(5));

        assertTrue(secondRan.get(), "a rejection by a peer must not skip the other validators");
        assertEquals(1, errors.size(), "an accepted validator contributes no error");
        assertEquals("camera", errors.get(0).validator());
        assertEquals("CAMERA_UNREACHABLE", errors.get(0).code());
        assertEquals("device offline", errors.get(0).message());
    }

    @Test
    void aValidatorReturningNoResultIsAStableValidatorFailure() {
        List<ConfigurationValidationError> errors = run(
                List.of(named("silent", (candidate, current, phase) -> null)),
                Duration.ofSeconds(5));

        assertEquals(1, errors.size());
        assertEquals("silent", errors.get(0).validator());
        assertEquals("VALIDATOR_FAILED", errors.get(0).code(),
                "a null verdict is a rejection, never a silent accept");
        assertEquals("validator returned no result", errors.get(0).message());
    }

    @Test
    void aValidatorThrowingAnErrorIsAStableValidatorFailureNotAnEscapingThrowable() {
        // Errors (not Exceptions) escape the invocation wrapper and surface as an
        // ExecutionException on the applying thread; they must still land as a coded rejection.
        List<ConfigurationValidationError> errors = run(
                List.of(named("hostile", (candidate, current, phase) -> {
                    throw new AssertionError("assertion\nblew\tup");
                })), Duration.ofSeconds(5));

        assertEquals(1, errors.size());
        assertEquals("hostile", errors.get(0).validator());
        assertEquals("VALIDATOR_FAILED", errors.get(0).code());
        assertEquals("assertion blew up", errors.get(0).message(),
                "the diagnostic is sanitized before it reaches an operator");
    }

    // ===================== the bounded deadline =====================

    @Test
    void anAlreadyLapsedDeadlineTimesOutEveryUnfinishedValidator() {
        List<ConfigurationValidationError> errors = run(List.of(
                named("first", blocksForever()),
                named("second", blocksForever())),
                Duration.ofNanos(1));

        assertEquals(2, errors.size(), "the deadline covers the whole validator set, not just one");
        assertEquals(List.of("first", "second"),
                errors.stream().map(ConfigurationValidationError::validator).toList());
        assertTrue(errors.stream().allMatch(e -> e.code().equals("VALIDATION_TIMEOUT")));
        assertEquals("configuration validation exceeded its bounded deadline",
                errors.get(0).message());
    }

    @Test
    void verdictsThatLandedBeforeTheDeadlineSurviveASiblingsTimeout() throws Exception {
        // "fast-reject" and "fast-error" both complete; "slow" then hangs past the deadline.
        CountDownLatch fastValidatorsDone = new CountDownLatch(2);
        List<ConfigurationValidationError> errors = run(List.of(
                named("slow", (candidate, current, phase) -> {
                    assertTrue(fastValidatorsDone.await(5, TimeUnit.SECONDS));
                    new CountDownLatch(1).await();
                    return ConfigurationCandidateValidator.Result.accept();
                }),
                named("fast-reject", (candidate, current, phase) -> {
                    try {
                        return ConfigurationCandidateValidator.Result.reject(
                                "ENDPOINT_BUSY", "in use");
                    } finally {
                        fastValidatorsDone.countDown();
                    }
                }),
                named("fast-error", (candidate, current, phase) -> {
                    fastValidatorsDone.countDown();
                    throw new AssertionError("late failure");
                })),
                Duration.ofMillis(300));

        assertEquals(3, errors.size(), "every validator is accounted for after the deadline");
        assertEquals("slow", errors.get(0).validator());
        assertEquals("VALIDATION_TIMEOUT", errors.get(0).code());
        assertEquals("fast-reject", errors.get(1).validator());
        assertEquals("ENDPOINT_BUSY", errors.get(1).code(),
                "a verdict delivered before the deadline is kept, not overwritten by the timeout");
        assertEquals("fast-error", errors.get(2).validator());
        assertEquals("VALIDATION_TIMEOUT", errors.get(2).code(),
                "a post-deadline failure falls back to the stable timeout code");
    }

    @Test
    void interruptingTheApplyingThreadIsReportedAndTheInterruptIsPreserved() throws Exception {
        CountDownLatch validatorEntered = new CountDownLatch(1);
        AtomicReference<List<ConfigurationValidationError>> result = new AtomicReference<>();
        AtomicBoolean interruptPreserved = new AtomicBoolean();

        Thread applier = new Thread(() -> {
            result.set(run(List.of(named("blocking", (candidate, current, phase) -> {
                validatorEntered.countDown();
                new CountDownLatch(1).await();
                return ConfigurationCandidateValidator.Result.accept();
            })), Duration.ofSeconds(30)));
            interruptPreserved.set(Thread.currentThread().isInterrupted());
        });
        applier.start();
        assertTrue(validatorEntered.await(5, TimeUnit.SECONDS));
        applier.interrupt();
        applier.join(Duration.ofSeconds(5));

        assertEquals(1, result.get().size());
        assertEquals("blocking", result.get().get(0).validator());
        assertEquals("VALIDATION_INTERRUPTED", result.get().get(0).code(),
                "an interrupted apply is a rejection, never a silent commit");
        assertTrue(interruptPreserved.get(),
                "the runner must re-assert the interrupt rather than swallow it");
    }

    // ===================== operator-safe diagnostics =====================

    @Test
    void rejectionDiagnosticsAreStrippedOfControlCharsAndCappedAt256Chars() {
        List<ConfigurationValidationError> errors = run(List.of(
                named("noisy", (candidate, current, phase) ->
                        ConfigurationCandidateValidator.Result.reject(
                                "BAD_ENDPOINT", "rtsp host\r\ndown")),
                named("verbose", (candidate, current, phase) ->
                        ConfigurationCandidateValidator.Result.reject(
                                "HUGE", "x".repeat(500)))),
                Duration.ofSeconds(5));

        assertEquals("rtsp host down", errors.get(0).message(),
                "control characters become spaces and runs of whitespace collapse");
        assertEquals(256, errors.get(1).message().length(),
                "a validator cannot flood the operator surface with an unbounded diagnostic");
        assertEquals("x".repeat(256), errors.get(1).message());
    }

    @Test
    void aNullDiagnosticBecomesAnEmptyMessageRatherThanTheStringNull() {
        List<ConfigurationValidationError> errors = run(
                List.of(named("terse", (candidate, current, phase) -> {
                    throw new IllegalStateException(); // no message
                })), Duration.ofSeconds(5));

        assertEquals("VALIDATOR_FAILED", errors.get(0).code());
        assertEquals("", errors.get(0).message());
    }

    // ===================== the reentrancy flag =====================

    @Test
    void inValidatorCallbackIsTrueOnlyWhileAValidatorIsRunning() {
        assertFalse(CandidateValidationRunner.inValidatorCallback(),
                "the flag is false on a thread that is not inside a validator");
        AtomicBoolean insideValidator = new AtomicBoolean();

        List<ConfigurationValidationError> errors = run(
                List.of(named("probe", (candidate, current, phase) -> {
                    insideValidator.set(CandidateValidationRunner.inValidatorCallback());
                    return ConfigurationCandidateValidator.Result.accept();
                })), Duration.ofSeconds(5));

        assertTrue(errors.isEmpty());
        assertTrue(insideValidator.get(),
                "a validator's own thread must report that it is inside a validator callback -"
                        + " this is what lets ConfigManager refuse a nested update");
        assertFalse(CandidateValidationRunner.inValidatorCallback(),
                "the applying thread is never marked as being inside a validator");
    }
}
