/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.utils;

import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.nio.file.FileSystems;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.nio.file.StandardWatchEventKinds;
import java.nio.file.WatchKey;
import java.nio.file.WatchService;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;

/**
 * Watches a <em>directory</em> (not a single file inode) and fires a {@link FileWatcher.FileChangeHandler}
 * callback on <em>any</em> create/modify/delete of an entry within it.
 *
 * <p>This is the Kubernetes-aware sibling of {@link FileWatcher}, built for the kubelet's atomic
 * ConfigMap update mechanism (DESIGN-subsystems §1, FR-CFG-2). A mounted ConfigMap is a directory of
 * symlinks: the user-visible {@code config.json} points at {@code ..data/config.json}, and {@code ..data}
 * is itself a symlink the kubelet swaps atomically (write a new timestamped dir, create {@code ..data_tmp}
 * pointing at it, then {@code rename(..data_tmp, ..data)}). Crucially:
 *
 * <ul>
 *   <li>an inotify/WatchService watch on the user-visible <em>file</em> fires once and dies after the
 *       swap (the inode it pointed at is gone — {@code IN_DELETE_SELF}); and</li>
 *   <li>the swap manifests as events on the {@code ..data}/{@code ..data_tmp} entries, <em>not</em> on
 *       {@code config.json}, so a name-filtered watch (as {@link FileWatcher} uses) never reloads.</li>
 * </ul>
 *
 * <p>Therefore this watcher (a) watches the mount directory, which persists across swaps; (b) reacts to
 * <em>every</em> entry event so the {@code ..data} swap triggers a reload; and (c) <b>re-arms</b> — if
 * the directory watch key is ever invalidated it re-registers, so the watch survives inode replacement
 * rather than silently going dead. The dotfile filter that prevents the projection artifacts from being
 * <em>parsed</em> as config lives in the provider; this watcher intentionally does not filter events,
 * because the {@code ..data} swap is exactly the signal it must act on.
 */
public class DirectoryWatcher extends Thread {

    protected static final Logger LOGGER = LogManager.getLogger(DirectoryWatcher.class);

    /** Backoff before re-registering after the directory watch key is invalidated (re-arm). */
    private static final long REARM_BACKOFF_MILLIS = 200L;

    private final Path dir;
    private final FileWatcher.FileChangeHandler handler;
    private final AtomicBoolean stop = new AtomicBoolean(false);

    /**
     * Creates a directory watcher.
     *
     * @param dir     the directory to watch (e.g. the ConfigMap mount point)
     * @param handler the callback invoked when any entry in {@code dir} changes
     */
    public DirectoryWatcher(Path dir, FileWatcher.FileChangeHandler handler) {
        this.dir = dir;
        this.handler = handler;
    }

    /** Convenience overload taking a string directory path. */
    public DirectoryWatcher(String dir, FileWatcher.FileChangeHandler handler) {
        this(Paths.get(dir), handler);
    }

    /**
     * Checks whether the watcher has been stopped.
     *
     * @return {@code true} once {@link #stopThread()} has been called
     */
    public boolean isStopped() {
        return stop.get();
    }

    /** Stops the watcher thread. */
    public void stopThread() {
        stop.set(true);
    }

    /** Executes the change handler callback. Invoked internally when directory changes are detected. */
    public void doOnChange() {
        handler.onChange();
    }

    @Override
    public void run() {
        // Outer loop = the re-arm loop: if the directory watch is lost (key invalidated, e.g. the
        // mount directory itself was replaced), drop out, back off, and re-register.
        boolean reconcileOnArm = false;
        while (!isStopped()) {
            try (WatchService watcher = FileSystems.getDefault().newWatchService()) {
                dir.register(watcher,
                        StandardWatchEventKinds.ENTRY_CREATE,
                        StandardWatchEventKinds.ENTRY_MODIFY,
                        StandardWatchEventKinds.ENTRY_DELETE);
                LOGGER.debug("DirectoryWatcher armed on {}", dir);

                // Reconcile after a RE-arm: a change can land in the gap between losing the old watch
                // and re-registering (the REARM_BACKOFF window), and a freshly-armed WatchService only
                // delivers events that occur AFTER it is armed — so re-read now, or that ConfigMap
                // update is silently lost (the "hot-reload dies after an update" failure mode).
                if (reconcileOnArm) {
                    LOGGER.debug("DirectoryWatcher reconciling state after re-arm on {}", dir);
                    doOnChange();
                }

                boolean rearm = watchLoop(watcher);
                if (!rearm) {
                    return; // stopped — exit cleanly without re-arming
                }
                LOGGER.warn("DirectoryWatcher key for {} invalidated; re-arming.", dir);
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                return;
            } catch (Throwable e) {
                // Registration failed (e.g. the directory does not exist yet during a swap window) or
                // an unexpected error occurred. Log and retry rather than silently dying.
                LOGGER.warn("DirectoryWatcher for {} could not arm ({}); retrying.", dir, e.toString());
            }

            // Any path reaching here is heading into a re-arm; reconcile right after the next arm.
            reconcileOnArm = true;
            // Back off before re-arming so a persistently-missing directory does not spin the CPU.
            if (!isStopped()) {
                try {
                    Thread.sleep(REARM_BACKOFF_MILLIS);
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                    return;
                }
            }
        }
    }

    /**
     * The inner poll loop for a single armed {@link WatchService}.
     *
     * @param watcher the armed watch service
     * @return {@code true} if the watch key was invalidated and the caller should re-arm;
     *         {@code false} if the watcher was stopped (no re-arm)
     * @throws InterruptedException if the polling thread is interrupted
     */
    private boolean watchLoop(WatchService watcher) throws InterruptedException {
        while (!isStopped()) {
            WatchKey key = watcher.poll(1, TimeUnit.SECONDS);
            if (key == null) {
                continue;
            }

            boolean changed = false;
            for (var event : key.pollEvents()) {
                if (event.kind() != StandardWatchEventKinds.OVERFLOW) {
                    // Any entry change (including the ..data symlink swap) triggers a re-read.
                    changed = true;
                }
            }
            if (changed) {
                doOnChange();
            }

            if (!key.reset()) {
                // The watched directory is gone/replaced — signal the outer loop to re-arm.
                return true;
            }
        }
        return false;
    }
}
