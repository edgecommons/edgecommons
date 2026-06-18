/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.utils;

import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Covers the {@link FileWatcher#run()} interruption branch: while the watcher thread is
 * blocked in {@code watcher.poll(1, TimeUnit.SECONDS)}, interrupting the thread raises an
 * {@link InterruptedException}, which the run loop catches, re-asserts the interrupt flag,
 * and returns from (FileWatcher L100, L102-103).
 *
 * <p>This is the only run-loop branch reachable deterministically without mocking the
 * JDK {@code WatchService}; the OVERFLOW (L116) and invalid-key (L134-135) branches are
 * not driven here because they cannot be provoked reliably across platforms.
 */
class FileWatcherInterruptTest {

    @Test
    void interruptingTheWatchThreadCausesItToReturn(@TempDir Path tempDir) throws Exception {
        Path target = tempDir.resolve("watched.json");
        Files.write(target, "{}".getBytes(StandardCharsets.UTF_8));

        FileWatcher watcher = new FileWatcher(target.toFile(), () -> { });
        watcher.start();

        // Let the thread reach the blocking poll(...) call, then interrupt it.
        Thread.sleep(300);
        watcher.interrupt();

        // The run loop catches InterruptedException and returns, so the thread must die.
        watcher.join(3000);
        assertFalse(watcher.isAlive(), "watch thread must terminate after being interrupted");
    }
}
