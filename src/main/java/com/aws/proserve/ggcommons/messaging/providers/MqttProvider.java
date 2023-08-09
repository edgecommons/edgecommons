package com.aws.proserve.ggcommons.messaging.providers;

import com.aws.proserve.ggcommons.messaging.Message;
import com.aws.proserve.ggcommons.messaging.MessagingProvider;
import com.aws.proserve.ggcommons.messaging.ReplyFuture;
import com.github.cliftonlabs.json_simple.Jsoner;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import org.eclipse.paho.client.mqttv3.*;

import java.nio.charset.StandardCharsets;
import java.util.HashMap;
import java.util.Map;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.Future;
import java.util.function.BiConsumer;

public class MqttProvider extends MessagingProvider
{
    protected static final Logger LOGGER = LogManager.getLogger(MqttProvider.class);
    HashMap<String,BiConsumer<String,Message>> subscriptionHandlers = new HashMap<>();
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
        public void messageArrived(String topic, MqttMessage message) throws Exception
        {
            Message msg;
            LOGGER.trace("Message received on topic '{}'", topic);
            String msgChars = new String(message.getPayload(), StandardCharsets.UTF_8);
            try {
                msg = Message.build(Jsoner.deserialize(msgChars));
            } catch (Exception e) {
                msg = Message.build(msgChars);
            }

            if (responseFutures.containsKey(topic)) {
                ReplyFuture future = responseFutures.get(topic);
                future.complete(msg);
                responseFutures.remove(topic);
                unsubscribe(topic);
            } else {
                final Message msg2 = msg;
                for (Map.Entry<String, BiConsumer<String, Message>> entry : provider.subscriptionHandlers.entrySet()) {
                    if (MessagingProvider.topicMatchesFilter(entry.getKey(), topic)) {
                        CompletableFuture.runAsync(() -> {
                            LOGGER.trace("Invoking callback for topic '{}'", topic);
                            entry.getValue().accept(topic, msg2);
                        });
                        break;
                    }
                }
            }
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

    @Override
    public void publish(String topic, Message message)
    {
        try
        {
            mqttClient.publish(topic, new MqttMessage(message.toString().getBytes()));
        }
        catch (MqttException e)
        {
            LOGGER.error("Failed to publish message on topic '{}' with header '{}' - {}",
                    topic, message.getHeader().toString(), e.toString());
        }
    }

    @Override
    public void subscribe(String topicFilter, BiConsumer<String, Message> callback)
    {
        subscriptionHandlers.put(topicFilter, callback);
        try
        {
            mqttClient.subscribe(topicFilter);
        }
        catch (MqttException e)
        {
            LOGGER.error("Failed to subscribe to topicFilter '{}' - {}", topicFilter, e.toString());
        }
    }

    @Override
    public void unsubscribe(String topicFilter)
    {
        try
        {
            mqttClient.unsubscribe(topicFilter);
            subscriptionHandlers.remove(topicFilter);
        }
        catch (Exception e)
        {
            LOGGER.warn("Problem unsubscribing from '{}'", topicFilter);
        }
    }

    @Override
    public ReplyFuture request(String topic, Message message)
    {
        String replyTo = message.makeRequest();
        ReplyFuture future = new ReplyFuture(replyTo);
        responseFutures.put(replyTo, future);
        subscribe(replyTo, null);
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
