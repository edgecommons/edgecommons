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

public class FileWatcher extends Thread {

    protected static final Logger LOGGER = LogManager.getLogger(FileWatcher.class);

    public interface FileChangeHandler {
        void onChange();
    }

    private final File file;
    private final AtomicBoolean stop = new AtomicBoolean(false);

    private final FileChangeHandler handler;

    public FileWatcher(String filePath, FileChangeHandler handler) {
        this.file = new File(filePath);
        this.handler = handler;
    }

    public FileWatcher(File file, FileChangeHandler handler) {
        this.file = file;
        this.handler = handler;
    }

    public boolean isStopped() { return stop.get(); }
    public void stopThread() { stop.set(true); }

    public void doOnChange() {
        handler.onChange();
    }

    @Override
    public void run() {
        try (WatchService watcher = FileSystems.getDefault().newWatchService())
        {
            Path path = file.toPath().getParent();
            path.register(watcher, StandardWatchEventKinds.ENTRY_MODIFY);
            while (!isStopped())
            {
                WatchKey key;

                try
                {
                    key = watcher.poll(25, TimeUnit.MILLISECONDS);
                }
                catch (InterruptedException e)
                {
                    return;
                }

                if (key == null)
                {
                    Thread.yield();
                    continue;
                }

                for (WatchEvent<?> event : key.pollEvents())
                {
                    WatchEvent.Kind<?> kind = event.kind();
                    @SuppressWarnings("unchecked")
                    WatchEvent<Path> ev = (WatchEvent<Path>) event;
                    Path filename = ev.context();

                    if (kind == StandardWatchEventKinds.OVERFLOW)
                    {
                        Thread.yield();
                        continue;
                    }
                    else if (kind == java.nio.file.StandardWatchEventKinds.ENTRY_MODIFY
                            && filename.toString().equals(file.getName()))
                    {
                        doOnChange();
                    }
                    boolean valid = key.reset();
                    if (!valid)
                    {
                        break;
                    }
                }
                Thread.yield();
            }
        }
        catch (Throwable e)
        {
            LOGGER.error("Error in FileWatcher {}. Ignoring.", e.getMessage());
        }
    }
}
