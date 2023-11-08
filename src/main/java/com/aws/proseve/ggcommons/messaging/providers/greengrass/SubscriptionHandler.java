package com.aws.proseve.ggcommons.messaging.providers.greengrass;


import com.aws.proseve.ggcommons.messaging.Message;
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
    }

    @Override
    public void run()
    {
        LOGGER.info("Starting queue monitoring for subscription on {}", topicFilter);
        while(true) {
            try
            {
                final QueueEntry entry = queue.take();
                if (entry.message == null && entry.topic == null)
                    break;
                executor.execute(() -> {
                    LOGGER.info("Invoking callback for topic '{}'", entry.topic);
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
