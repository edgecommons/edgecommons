/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.messaging.providers.greengrass;


import com.aws.proserve.ggcommons.messaging.Message;
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
    LinkedBlockingQueue<QueueEntry> queue = new LinkedBlockingQueue<>();
    ExecutorService executor;
    private volatile boolean shutdown = false;

    public SubscriptionHandler(String topicFilter, BiConsumer<String, Message> callback, int maxConcurrency)
    {
        this.topicFilter = topicFilter;
        this.callback = callback;
        this.maxConcurrency = maxConcurrency;
        if (maxConcurrency <= 0)
        {
            executor = Executors.newCachedThreadPool();

        } else {
            executor = new ThreadPoolExecutor(0, maxConcurrency,60L, TimeUnit.SECONDS,
                    new LinkedBlockingQueue<>());
        }
        new Thread(this).start();
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
            queue.add(new QueueEntry(parsedMessage.getA(), parsedMessage.getB()));
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
                executor.execute(() -> {
                    LOGGER.trace("Invoking callback for topic '{}'", entry.topic);
                    callback.accept(entry.topic, entry.message);
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
