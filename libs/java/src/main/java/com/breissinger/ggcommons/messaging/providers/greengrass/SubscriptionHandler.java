/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons.messaging.providers.greengrass;


import com.breissinger.ggcommons.messaging.Message;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import oshi.util.tuples.Pair;
import software.amazon.awssdk.eventstreamrpc.StreamResponseHandler;

import java.util.concurrent.*;
import java.util.function.BiConsumer;

public abstract class SubscriptionHandler<T> implements Runnable, StreamResponseHandler<T>
{
    protected static final Logger LOGGER = LogManager.getLogger(SubscriptionHandler.class);

    protected static class QueueEntry
    {
        public String topic;
        public Message message;

        QueueEntry(String topic, Message message)
        {
            this.topic = topic;
            this.message = message;
        }
    }

    protected String topicFilter;
    protected BiConsumer<String, Message> callback;
    protected int maxConcurrency;
    protected int maxMessages;
    LinkedBlockingQueue<QueueEntry> queue;
    ExecutorService executor;
    private final Semaphore concurrencyLimit;
    private volatile boolean shutdown = false;

    public SubscriptionHandler(String topicFilter, BiConsumer<String, Message> callback, int maxConcurrency, int maxMessages)
    {
        this.topicFilter = topicFilter;
        this.callback = callback;
        this.maxConcurrency = maxConcurrency;
        this.maxMessages = maxMessages;
        // Bounded queue (drop on overflow) when maxMessages > 0, else unbounded — parity with
        // the Rust (bounded mpsc) / TS (drop-on-overflow) providers.
        this.queue = maxMessages > 0 ? new LinkedBlockingQueue<>(maxMessages) : new LinkedBlockingQueue<>();
        // One virtual thread per callback (callbacks block on IPC / IoT Core / CloudWatch
        // I/O). A positive maxConcurrency is enforced with a Semaphore, preserving the
        // bounded-concurrency contract without a fixed platform-thread pool.
        executor = Executors.newVirtualThreadPerTaskExecutor();
        concurrencyLimit = maxConcurrency > 0 ? new Semaphore(maxConcurrency) : null;
        new Thread(this, "gg-sub-" + topicFilter).start();
    }

    public void shutdown() {
        shutdown = true;
        queue.offer(new QueueEntry(null, null)); // Poison pill
        executor.shutdown();
        try {
            if (!executor.awaitTermination(5, TimeUnit.SECONDS)) {
                executor.shutdownNow();
            }
        } catch (InterruptedException e) {
            executor.shutdownNow();
            Thread.currentThread().interrupt();
        }
    }
    
    abstract Pair<String,Message> parseRawPayload(T rawPayload);

    @Override
    public void onStreamEvent(T rawMessage)
    {
        Pair<String, Message> parsedMessage = parseRawPayload(rawMessage);
        if (parsedMessage != null)
        {
            // Non-blocking enqueue: a full bounded queue drops the message with a warning rather
            // than blocking the IPC stream thread (parity with Rust/TS).
            if (!queue.offer(new QueueEntry(parsedMessage.getA(), parsedMessage.getB())))
            {
                LOGGER.warn("Subscription queue full (maxMessages={}) for '{}'; dropping message on {}",
                        maxMessages, topicFilter, parsedMessage.getA());
            }
        }
    }

    @Override
    public boolean onStreamError(Throwable throwable)
    {
        LOGGER.error("Error on stream for subscription to topicFilter {}: {}", topicFilter, throwable.toString());
        return false;
    }

    @Override
    public void onStreamClosed()
    {
        LOGGER.info("Stream for subscription to topicFilter {} closed (unsubscribed)", topicFilter);
        shutdown();
    }

    @Override
    public void run()
    {
        LOGGER.info("Starting queue monitoring for subscription on {}", topicFilter);
        while(!shutdown) {
            try
            {
                final QueueEntry entry = queue.take();
                if (entry.message == null && entry.topic == null)
                    break;
                if (concurrencyLimit != null)
                {
                    // Backpressure: block the drain thread until a permit frees, so at
                    // most maxConcurrency callbacks run (and are in flight) at once.
                    concurrencyLimit.acquireUninterruptibly();
                }
                executor.execute(() -> {
                    try
                    {
                        LOGGER.trace("Invoking callback for topic '{}'", entry.topic);
                        callback.accept(entry.topic, entry.message);
                    }
                    catch (Throwable t)
                    {
                        // A subscription callback must never throw out of this task: an uncaught
                        // exception here kills the worker thread and (under crash/restart churn) can
                        // surface in the eventstream RPC layer and destabilize nucleus IPC. Contain
                        // and log it so one bad message can't take down the subscription or the core.
                        LOGGER.error("Subscription callback for topic '{}' threw; suppressed: {}",
                                entry.topic, t.toString(), t);
                    }
                    finally
                    {
                        if (concurrencyLimit != null)
                        {
                            concurrencyLimit.release();
                        }
                    }
                });
            }
            catch (InterruptedException e)
            {
                LOGGER.warn("Subscription processing for {} interrupted. Restarting. Exception: {}",
                        topicFilter, e.getMessage());
            }
        }
        LOGGER.info("Queue monitoring stopped for subscription on {}", topicFilter);
    }
}
