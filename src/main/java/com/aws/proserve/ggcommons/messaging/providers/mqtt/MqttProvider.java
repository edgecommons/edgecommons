package com.aws.proserve.ggcommons.messaging.providers.mqtt;

import com.aws.proserve.ggcommons.messaging.Message;
import com.aws.proserve.ggcommons.messaging.MessagingProvider;
import com.aws.proserve.ggcommons.messaging.ReplyFuture;
import com.google.gson.Gson;
import com.google.gson.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import org.eclipse.paho.client.mqttv3.IMqttDeliveryToken;
import org.eclipse.paho.client.mqttv3.MqttCallback;
import org.eclipse.paho.client.mqttv3.*;
import software.amazon.awssdk.aws.greengrass.model.QOS;
import javax.net.ssl.SSLContext;
import javax.net.ssl.SSLSocketFactory;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.security.KeyStore;
import java.security.PrivateKey;
import java.security.cert.CertificateFactory;
import java.util.HashMap;
import java.util.UUID;
import java.util.concurrent.*;
import java.util.function.BiConsumer;
import java.io.*;
import java.nio.file.*;
import java.security.cert.*;
import javax.net.ssl.*;


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
        public int maxConcurrency;
        public LinkedBlockingQueue<QueueEntry> queue;
        ExecutorService executor;

        SubscriptionProcessor(String topicFilter, BiConsumer<String, Message> callback, int maxConcurrency)
        {
            super();
            this.topicFilter = topicFilter;
            this.callback = callback;
            this.maxConcurrency = maxConcurrency;
            this.queue = new LinkedBlockingQueue<>();
            if (maxConcurrency <= 0)
            {
                executor = Executors.newCachedThreadPool();

            } else {
                executor = new ThreadPoolExecutor(0, maxConcurrency,60L, TimeUnit.SECONDS,
                        new LinkedBlockingQueue<>());
            }
            new Thread(this).start();
        }

        @Override
        public void run()
        {
            LOGGER.trace("Started queue monitoring for subscription on {}", topicFilter);
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
                            LOGGER.debug("Invoking callback for topic '{}'", topic);
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
            LOGGER.trace("Queue monitoring stopped for subscription on {}", topicFilter);
        }
    }

    HashMap<String,SubscriptionProcessor> subscriptionProcessors = new HashMap<>();

    String host;
    int port;
    String certFolder;
    String clientId;
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
                msg = Message.build(new Gson().fromJson(msgChars, JsonObject.class));
            } catch (Exception e) {
                msg = Message.build(msgChars);
            }
            SubscriptionProcessor subscriptionProcessor = subscriptionProcessors.get(topic);
            if (subscriptionProcessor == null) {
                for (SubscriptionProcessor processor : subscriptionProcessors.values()) {
                    if (topicMatchesFilter(processor.topicFilter, topic)) {
                        subscriptionProcessor = processor;
                        break;
                    }
                }
            }
            if (subscriptionProcessor != null) {
                subscriptionProcessor.queue.add(new QueueEntry(topic, msg));
            } else {
                LOGGER.warn("No callback registered for topic '{}'. Ignoring message.", topic);
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
        certFolder = messagingArgs.length > 3 ? messagingArgs[3] : null;
        this.clientId = clientId;
        String prefix = certFolder != null ? "ssl" : "tcp";
        String uri =  String.format("%s://%s:%d", prefix, host, port);
        try
        {
            mqttClient = new MqttClient(uri, this.clientId);
            mqttClient.setCallback(new EventCallback(this));
            MqttConnectOptions connOpts = new MqttConnectOptions();
            if (certFolder != null)
            {
                String caCertFile = String.format("%s/root-CA.crt", certFolder);
                String deviceCertFile = String.format("%s/%s.cert.pem", certFolder, this.clientId);
                String devicePrivateKeyFile = String.format("%s/%s.private.key", certFolder, this.clientId);
                SSLSocketFactory socketFactory = getSocketFactory(caCertFile,deviceCertFile,devicePrivateKeyFile);
                if (socketFactory != null)
                {
                    connOpts.setSocketFactory(socketFactory);
                    LOGGER.info("Attempting to connect to MQTT broker at {}", uri);
                    LOGGER.info("       ...using private key file: {}", devicePrivateKeyFile);
                    LOGGER.info("       ...using device cert file: {}", deviceCertFile);
                    LOGGER.info("       ...using CA Cert file: {}", caCertFile);
                    LOGGER.info("       ...using client ID:{}", clientId);
                }
                else
                    LOGGER.warn("Unable to load cert/key files. Attempting to connect without credentials.");
            }
            connOpts.setCleanSession(true);
            connOpts.setMaxInflight(250);
            connOpts.setConnectionTimeout(10);
            mqttClient.connect(connOpts);
            LOGGER.info("Connected to MQTT broker {}", uri);
        }
        catch (MqttException e)
        {
            LOGGER.fatal("Failed to connect to MQTT broker at {} - {}", uri, e.toString());
            System.exit(4);
        }
    }

    private static SSLSocketFactory getSocketFactory(final String caCrtFile, final String crtFile, final String keyFile)
    {
        SSLSocketFactory retVal = null;
        try
        {
            // load CA certificate
            X509Certificate caCert = (X509Certificate) CertificateFactory.getInstance("X509").
                                                                         generateCertificate(new ByteArrayInputStream(Files.readAllBytes(Paths.get(caCrtFile))));

            // load client certificate
            X509Certificate cert = (X509Certificate) CertificateFactory.getInstance("X509").
                                                     generateCertificate(new ByteArrayInputStream(Files.readAllBytes(Paths.get(crtFile))));

            // load client private key
            PrivateKey privateKey = PrivateKeyReader.getPrivateKey(keyFile);

            // CA certificate is used to authenticate server
            KeyStore caKs = KeyStore.getInstance(KeyStore.getDefaultType());
            caKs.load(null, null);
            caKs.setCertificateEntry("ca-certificate", caCert);
            TrustManagerFactory tmf = TrustManagerFactory.getInstance(TrustManagerFactory.getDefaultAlgorithm());
            tmf.init(caKs);

            // client key and certificates are sent to server so it can authenticate us
            char[] password = UUID.randomUUID().toString().toCharArray();
            KeyStore ks = KeyStore.getInstance(KeyStore.getDefaultType());
            ks.load(null, null);
            ks.setCertificateEntry("certificate", cert);
            ks.setKeyEntry("private-key", privateKey, password, new java.security.cert.Certificate[]{cert});
            KeyManagerFactory kmf = KeyManagerFactory.getInstance(KeyManagerFactory.getDefaultAlgorithm());
            kmf.init(ks, password);

            // finally, create SSL socket factory
            SSLContext context = SSLContext.getInstance("TLS");
            context.init(kmf.getKeyManagers(), tmf.getTrustManagers(), null);
            retVal = context.getSocketFactory();
        } catch (Exception e) {
            LOGGER.error("Failed to load certificates - {}", e.toString());
        }
        return retVal;
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

    @Override
    public void publishRaw(String topic, JsonObject payload)
    {
        try
        {
            MqttMessage msg = new MqttMessage(payload.toString().getBytes());
            mqttClient.publish(topic, msg);
        }
        catch (MqttException e)
        {
            LOGGER.error("Failed to publish raw message on topic '{}' - {}",
                    topic, e.toString());
        }
    }

    private void internalSubscribe(String topicFilter, BiConsumer<String, Message> callback, QOS qos, int maxConcurrency)
    {
        SubscriptionProcessor subProcessor = new SubscriptionProcessor(topicFilter, callback, maxConcurrency);
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

    public void subscribe(String topicFilter, BiConsumer<String, Message> callback, int maxConcurrency)
    {
        internalSubscribe(topicFilter, callback, QOS.AT_LEAST_ONCE, maxConcurrency);
    }

    @Override
    public void subscribeToIoTCore(String topicFilter, BiConsumer<String, Message> callback, QOS qos,
                                   int maxConcurrency)
    {
        String adjustedTopicFilter = "iotcore/" + topicFilter;
        internalSubscribe(adjustedTopicFilter, callback, qos, maxConcurrency);
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
        subscribe(replyTo, null, 1);
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

    @Override
    public ReplyFuture requestFromIoTCore(String topic, Message message)
    {
        String replyTo = message.makeRequest();
        ReplyFuture future = new ReplyFuture(replyTo);
        responseFutures.put(replyTo, future);
        internalSubscribe(replyTo, null, QOS.AT_MOST_ONCE,1);
        publishToIoTCore(topic, message, QOS.AT_MOST_ONCE);
        return future;
    }

    @Override
    public void cancelRequestFromIoTCore(ReplyFuture future)
    {
        unsubscribeFromIoTCore(future.replyTopic);
        responseFutures.remove(future.replyTopic);
        future.complete(null);
    }

    @Override
    public void replyToIoTCore(Message request, Message reply)
    {
        reply.setCorrelationId(request.getHeader().getCorrelationId());
        publishToIoTCore(request.getHeader().getReplyTo(), reply, QOS.AT_MOST_ONCE);
    }

    @Override
    public Object getNativeClient()
    {
        return mqttClient;
    }
}
