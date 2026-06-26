/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.utils;

import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Unit tests for {@link DirectoryWatcher}: the basic stop/callback plumbing, that it fires on a
 * directory entry change, and — most importantly — that it <b>re-arms</b> (retries registration) when
 * its target directory does not exist yet, then begins delivering events once the directory appears.
 */
class DirectoryWatcherTest {

    @Test
    void doOnChangeInvokesHandlerAndStopFlagIsHonored() {
        AtomicInteger count = new AtomicInteger();
        DirectoryWatcher w = new DirectoryWatcher("/nonexistent-never-watched", count::incrementAndGet);
        assertFalse(w.isStopped());
        w.doOnChange();
        assertEquals(1, count.get());
        w.stopThread();
        assertTrue(w.isStopped());
    }

    @Test
    void firesOnEntryChange(@TempDir Path dir) throws Exception {
        CountDownLatch latch = new CountDownLatch(1);
        DirectoryWatcher w = new DirectoryWatcher(dir, latch::countDown);
        w.setDaemon(true);
        w.start();
        try {
            Thread.sleep(2_000); // let the watch arm before mutating (avoids the create-before-register race)
            Files.write(dir.resolve("a.txt"), "hi".getBytes(StandardCharsets.UTF_8));
            assertTrue(latch.await(20, TimeUnit.SECONDS), "watcher should fire on a directory change");
        } finally {
            w.stopThread();
        }
    }

    @Test
    void reArmsWhenDirectoryAppearsLater(@TempDir Path parent) throws Exception {
        // Watch a directory that does not exist yet: registration fails, the watcher backs off and
        // retries (re-arm). Once the directory and an entry appear, it begins delivering events.
        Path target = parent.resolve("late-mount");
        CountDownLatch latch = new CountDownLatch(1);
        DirectoryWatcher w = new DirectoryWatcher(target, latch::countDown);
        w.setDaemon(true);
        w.start();
        try {
            // Let a few register-retry cycles elapse before the directory exists.
            Thread.sleep(500);
            Files.createDirectory(target);
            Thread.sleep(2_000); // let the watch arm on the newly-created directory before mutating
            Files.write(target.resolve("config.json"), "{}".getBytes(StandardCharsets.UTF_8));
            assertTrue(latch.await(20, TimeUnit.SECONDS),
                    "watcher should re-arm and fire once the directory appears");
        } finally {
            w.stopThread();
        }
    }

    @Test
    void stopsCleanlyWithoutFiringWhenNothingChanges(@TempDir Path dir) throws Exception {
        AtomicInteger count = new AtomicInteger();
        DirectoryWatcher w = new DirectoryWatcher(dir, count::incrementAndGet);
        w.setDaemon(true);
        w.start();
        Thread.sleep(300);
        w.stopThread();
        w.join(5_000);
        assertFalse(w.isAlive(), "watcher thread should exit after stopThread()");
        assertEquals(0, count.get());
    }
}
