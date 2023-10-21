package com.aws.proserve.ggcommons.messaging.providers;

import com.aws.proserve.ggcommons.messaging.Message;
import com.aws.proserve.ggcommons.messaging.MessagingProvider;
import com.aws.proserve.ggcommons.messaging.ReplyFuture;
import com.github.cliftonlabs.json_simple.Jsoner;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import org.eclipse.paho.client.mqttv3.*;
import software.amazon.awssdk.aws.greengrass.model.QOS;

import java.nio.charset.StandardCharsets;
import java.util.HashMap;
import java.util.concurrent.*;
import java.util.function.BiConsumer;


public class MqttProvider extends MessagingProvider
{
    protected static final Logger LOGGER = LogManager.getLogger(MqttProvider.class);


    private static class QueueEntry
    {
        public String topic;
        public Message message;

        QueueEntry(String topic, Message message)
        {
            this.topic = topic;
            this.message = message;
        }
    }

    private class SubscriptionProcessor implements Runnable
    {
        public String topicFilter;
        public BiConsumer<String, Message> callback;
        public boolean serialize;
        public LinkedBlockingQueue<QueueEntry> queue;
        ExecutorService executor;

        SubscriptionProcessor(String topicFilter, BiConsumer<String, Message> callback, boolean serialize)
        {
            super();
            this.topicFilter = topicFilter;
            this.callback = callback;
            this.serialize = serialize;
            this.queue = new LinkedBlockingQueue<>();
            if (serialize) {
                executor = Executors.newSingleThreadExecutor();
            } else {
                executor = Executors.newCachedThreadPool();
            }
            new Thread(this).start();
        }

        @Override
        public void run()
        {
            LOGGER.info("Started queue monitoring for subscription on {}", topicFilter);
            while(true)
            {
                try
                {
                    final QueueEntry entry = queue.take();
                    if (entry.message == null && entry.topic == null) {
                        break;
                    }
                    final String topic = entry.topic.replaceFirst("^iotcore/", "");
                    if (responseFutures.containsKey(entry.topic)) {
                        ReplyFuture future = responseFutures.get(topic);
                        future.complete(entry.message);
                        responseFutures.remove(topic);
                        unsubscribe(topic);
                    } else {
                        executor.execute(() -> {
                            LOGGER.info("Invoking callback for topic '{}'", topic);
                            callback.accept(topic, entry.message);
                        });
                    }
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

    HashMap<String,SubscriptionProcessor> subscriptionProcessors = new HashMap<>();

    String host;
    int port;
    MqttClient mqttClient;
    HashMap<String, ReplyFuture> responseFutures = new HashMap<>();

    private class EventCallback implements MqttCallback
    {
        MqttProvider provider;

        public EventCallback(MqttProvider mqttProvider)
        {
            this.provider = mqttProvider;
        }

        @Override
        public void connectionLost(Throwable cause)
        {
            // TODO: attempt reconnect here
            LOGGER.error("Connection to MQTT broker lost - {}", cause.toString());
        }

        @Override
        public void messageArrived(String topic, MqttMessage message)
        {
            Message msg;
            LOGGER.trace("Message received on topic '{}'", topic);
            String msgChars = new String(message.getPayload(), StandardCharsets.UTF_8);
            try {
                msg = Message.build(Jsoner.deserialize(msgChars));
            } catch (Exception e) {
                msg = Message.build(msgChars);
            }
            subscriptionProcessors.get(topic).queue.add(new QueueEntry(topic, msg));
        }

        @Override
        public void deliveryComplete(IMqttDeliveryToken token)
        {
            LOGGER.trace("Message delivery complete: {}", token.getMessageId());
        }
    }

    public MqttProvider(String[] messagingArgs, String clientId)
    {
        super(messagingArgs);
        host = messagingArgs.length > 1 ? messagingArgs[1] : "localhost";
        port = messagingArgs.length > 2 ? Integer.parseInt(messagingArgs[2]) : 1883;
        try
        {
            mqttClient = new MqttClient(String.format("tcp://%s:%d", host, port), clientId);
            mqttClient.setCallback(new EventCallback(this));
            MqttConnectOptions connOpts = new MqttConnectOptions();
            connOpts.setCleanSession(true);
            connOpts.setMaxInflight(250);
            mqttClient.connect();
            LOGGER.info("Connected to MQTT broker tcp://{}:{}", host, port);
        }
        catch (MqttException e)
        {
            LOGGER.fatal("Failed to connect to MQTT broker at tcp://{}:{} - {}", host, port, e.toString());
            System.exit(4);
        }
    }

    private void internalPublish(String topic, Message message, QOS qos) {
        try
        {
            MqttMessage msg = new MqttMessage(message.toString().getBytes());
            msg.setQos(qos.ordinal());
            mqttClient.publish(topic, msg);
        }
        catch (MqttException e)
        {
            LOGGER.error("Failed to publish message on topic '{}' with header '{}' - {}",
                    topic, message.getHeader().toString(), e.toString());
        }
    }

    @Override
    public void publish(String topic, Message message)
    {
        internalPublish(topic, message, QOS.AT_LEAST_ONCE);
    }

    @Override
    public void publishToIoTCore(String topic, Message message, QOS qos)
    {
        String adjustedTopic = "iotcore/" + topic;
        internalPublish(adjustedTopic, message, qos);
    }

    private void internalSubscribe(String topicFilter, BiConsumer<String, Message> callback, QOS qos, boolean serializeProcessing)
    {
        SubscriptionProcessor subProcessor = new SubscriptionProcessor(topicFilter, callback, serializeProcessing);
        subscriptionProcessors.put(topicFilter, subProcessor);
        try
        {
            mqttClient.subscribe(topicFilter, qos.ordinal());
        }
        catch (MqttException e)
        {
            LOGGER.error("Failed to subscribe to topicFilter '{}': {}", topicFilter, e.toString());
        }
    }

    public void subscribe(String topicFilter, BiConsumer<String, Message> callback, boolean serializeProcessing)
    {
        internalSubscribe(topicFilter, callback, QOS.AT_LEAST_ONCE, serializeProcessing);
    }

    @Override
    public void subscribeToIoTCore(String topicFilter, BiConsumer<String, Message> callback, QOS qos,
                                   boolean serializeProcessing)
    {
        String adjustedTopicFilter = "iotcore/" + topicFilter;
        internalSubscribe(adjustedTopicFilter, callback, qos, serializeProcessing);
    }

    @Override
    public void unsubscribe(String topicFilter)
    {
        try
        {
            SubscriptionProcessor subProcessor = subscriptionProcessors.get(topicFilter);
            if (subProcessor != null) {
                subProcessor.queue.put(new QueueEntry(null, null));
                subscriptionProcessors.remove(topicFilter);
                mqttClient.unsubscribe(topicFilter);
            }
        }
        catch (Exception e)
        {
            LOGGER.warn("Problem unsubscribing from '{}': {}", topicFilter, e.getMessage());
        }
    }

    @Override
    public void unsubscribeFromIoTCore(String topicFilter)
    {
        String adjustedTopicFilter = "iotcore/" + topicFilter;
        unsubscribe(adjustedTopicFilter);
    }

    @Override
    public ReplyFuture request(String topic, Message message)
    {
        String replyTo = message.makeRequest();
        ReplyFuture future = new ReplyFuture(replyTo);
        responseFutures.put(replyTo, future);
        subscribe(replyTo, null, true);
        publish(topic, message);
        return future;
    }

    @Override
    public void cancelRequest(ReplyFuture future)
    {
        unsubscribe(future.replyTopic);
        responseFutures.remove(future.replyTopic);
        future.complete(null);
    }

    @Override
    public void reply(Message request, Message reply)
    {
        reply.setCorrelationId(request.getHeader().getCorrelationId());
        publish(request.getHeader().getReplyTo(), reply);
    }
}
