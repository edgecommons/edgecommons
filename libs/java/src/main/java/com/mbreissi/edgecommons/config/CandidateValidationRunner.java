/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.config;

import com.google.gson.JsonObject;

import java.time.Duration;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;

/** Internal bounded runner for side-effect-free candidate validators. */
final class CandidateValidationRunner {
    private static final int MAX_THREADS = 4;
    private static final int MAX_DIAGNOSTIC_CHARS = 256;
    private static final ThreadLocal<Boolean> IN_VALIDATOR_CALLBACK =
            ThreadLocal.withInitial(() -> false);

    record NamedValidator(String name, ConfigurationCandidateValidator validator) { }

    private CandidateValidationRunner() { }

    static boolean inValidatorCallback() {
        return IN_VALIDATOR_CALLBACK.get();
    }

    static List<ConfigurationValidationError> validate(
            List<NamedValidator> validators,
            JsonObject candidate,
            JsonObject redactedCurrent,
            ConfigurationValidationPhase phase,
            Duration timeout) {
        if (validators.isEmpty()) {
            return List.of();
        }

        ExecutorService executor = Executors.newFixedThreadPool(
                Math.min(MAX_THREADS, validators.size()), runnable -> {
                    Thread thread = new Thread(runnable, "edgecommons-config-validator");
                    thread.setDaemon(true);
                    return thread;
                });
        List<Future<ConfigurationValidationError>> futures = new ArrayList<>();
        for (NamedValidator named : validators) {
            futures.add(executor.submit(() -> invoke(named, candidate, redactedCurrent, phase)));
        }

        long deadline = System.nanoTime() + timeout.toNanos();
        List<ConfigurationValidationError> errors = new ArrayList<>();
        try {
            for (int index = 0; index < futures.size(); index++) {
                Future<ConfigurationValidationError> future = futures.get(index);
                long remaining = deadline - System.nanoTime();
                if (remaining <= 0) {
                    collectAfterDeadline(validators, futures, index, errors);
                    break;
                }
                try {
                    ConfigurationValidationError error = future.get(remaining, TimeUnit.NANOSECONDS);
                    if (error != null) {
                        errors.add(error);
                    }
                } catch (TimeoutException e) {
                    collectAfterDeadline(validators, futures, index, errors);
                    break;
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                    errors.add(new ConfigurationValidationError(
                            validators.get(index).name(), "VALIDATION_INTERRUPTED",
                            "configuration validation was interrupted"));
                    break;
                } catch (ExecutionException e) {
                    errors.add(failed(validators.get(index).name(), e.getCause()));
                }
            }
        } finally {
            futures.forEach(future -> future.cancel(true));
            executor.shutdownNow();
        }
        return List.copyOf(errors);
    }

    private static ConfigurationValidationError invoke(
            NamedValidator named,
            JsonObject candidate,
            JsonObject redactedCurrent,
            ConfigurationValidationPhase phase) {
        try {
            IN_VALIDATOR_CALLBACK.set(true);
            ConfigurationCandidateValidator.Result result = named.validator().validate(
                    candidate.deepCopy(),
                    redactedCurrent == null ? null : redactedCurrent.deepCopy(),
                    phase);
            if (result == null) {
                return new ConfigurationValidationError(named.name(), "VALIDATOR_FAILED",
                        "validator returned no result");
            }
            return result.accepted() ? null : new ConfigurationValidationError(
                    named.name(), result.code(), sanitize(result.message()));
        } catch (Exception e) {
            return failed(named.name(), e);
        } finally {
            IN_VALIDATOR_CALLBACK.remove();
        }
    }

    private static void collectAfterDeadline(
            List<NamedValidator> validators,
            List<Future<ConfigurationValidationError>> futures,
            int first,
            List<ConfigurationValidationError> errors) {
        for (int index = first; index < futures.size(); index++) {
            Future<ConfigurationValidationError> future = futures.get(index);
            if (future.isDone()) {
                try {
                    ConfigurationValidationError error = future.get();
                    if (error != null) {
                        errors.add(error);
                    }
                    continue;
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                } catch (ExecutionException ignored) {
                    // The invocation wrapper normally converts failures; use the stable fallback.
                }
            }
            errors.add(new ConfigurationValidationError(
                    validators.get(index).name(), "VALIDATION_TIMEOUT",
                    "configuration validation exceeded its bounded deadline"));
        }
    }

    private static ConfigurationValidationError failed(String name, Throwable error) {
        String detail = error == null ? "validator failed" : error.getMessage();
        return new ConfigurationValidationError(name, "VALIDATOR_FAILED", sanitize(detail));
    }

    static String sanitize(String message) {
        String source = message == null ? "" : message;
        StringBuilder safe = new StringBuilder(Math.min(source.length(), MAX_DIAGNOSTIC_CHARS));
        for (int index = 0; index < source.length() && safe.length() < MAX_DIAGNOSTIC_CHARS; index++) {
            char c = source.charAt(index);
            safe.append(Character.isISOControl(c) ? ' ' : c);
        }
        return safe.toString().replaceAll("\\s+", " ").trim();
    }
}
