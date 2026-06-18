/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.utils;

import com.aws.proserve.ggcommons.metrics.MetricEmitter;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.io.File;
import java.nio.file.*;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;


// Following code taken from https://stackoverflow.com/questions/16251273/can-i-watch-for-single-file-change-with-watchservice-not-the-whole-directory
// with minor changes for use here.

/**
 * Utility class that monitors file changes in the filesystem.
 * Provides functionality to watch files or directories and trigger callbacks when changes occur.
 */
public class FileWatcher extends Thread {

    protected static final Logger LOGGER = LogManager.getLogger(FileWatcher.class);

    /**
     * Interface for handling file change events.
     * Implementations should define the action to take when a file changes.
     */
    public interface FileChangeHandler {
        void onChange();
    }

    private final File file;
    private final AtomicBoolean stop = new AtomicBoolean(false);

    private final FileChangeHandler handler;

    /**
     * Creates a file watcher for the specified file path.
     *
     * @param filePath The path to the file to watch
     * @param handler The handler to call when changes are detected
     */
    public FileWatcher(String filePath, FileChangeHandler handler) {
        this.file = new File(filePath);
        this.handler = handler;
    }

    /**
     * Creates a file watcher for the specified file.
     *
     * @param file The file to watch
     * @param handler The handler to call when changes are detected
     */
    public FileWatcher(File file, FileChangeHandler handler) {
        this.file = file;
        this.handler = handler;
    }

    /**
     * Checks if the file watcher has been stopped.
     *
     * @return true if the watcher has been stopped, false otherwise
     */
    public boolean isStopped() { return stop.get(); }
    /**
     * Stops the file watcher thread.
     */
    public void stopThread() { stop.set(true); }

    /**
     * Executes the change handler callback.
     * This is called internally when file changes are detected.
     */
    public void doOnChange() {
        handler.onChange();
    }

    @Override
    public void run() {
        try (WatchService watcher = FileSystems.getDefault().newWatchService())
        {
            Path path = file.toPath().toAbsolutePath().getParent();
            // Watch MODIFY (in-place writes) and CREATE (atomic save-and-rename: many editors
            // and config writers write a temp file then rename it onto the target).
            path.register(watcher,
                    StandardWatchEventKinds.ENTRY_MODIFY,
                    StandardWatchEventKinds.ENTRY_CREATE);
            while (!isStopped())
            {
                WatchKey key;

                try
                {
                    key = watcher.poll(1, TimeUnit.SECONDS);
                }
                catch (InterruptedException e)
                {
                    Thread.currentThread().interrupt();
                    return;
                }

                if (key == null)
                {
                    continue;
                }

                for (WatchEvent<?> event : key.pollEvents())
                {
                    WatchEvent.Kind<?> kind = event.kind();
                    if (kind == StandardWatchEventKinds.OVERFLOW)
                    {
                        continue;
                    }
                    @SuppressWarnings("unchecked")
                    WatchEvent<Path> ev = (WatchEvent<Path>) event;
                    Path filename = ev.context();

                    if ((kind == StandardWatchEventKinds.ENTRY_MODIFY
                            || kind == StandardWatchEventKinds.ENTRY_CREATE)
                            && filename != null
                            && filename.toString().equals(file.getName()))
                    {
                        doOnChange();
                    }
                }

                boolean valid = key.reset();
                if (!valid)
                {
                    LOGGER.warn("FileWatcher watch key for {} is no longer valid; stopping watch.", file);
                    break;
                }
            }
        }
        catch (Throwable e)
        {
            // Log the full exception (with stack trace) rather than swallowing it silently.
            LOGGER.error("FileWatcher for {} terminated due to an unrecoverable error.", file, e);
        }
    }
}
