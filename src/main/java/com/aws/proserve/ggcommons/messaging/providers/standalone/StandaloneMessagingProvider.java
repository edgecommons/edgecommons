/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.messaging.providers.standalone;

import com.aws.proserve.ggcommons.messaging.Message;
import com.aws.proserve.ggcommons.messaging.MessageBuilder;
import com.aws.proserve.ggcommons.messaging.MessagingConfiguration;
import com.aws.proserve.ggcommons.messaging.MessagingProvider;
import com.aws.proserve.ggcommons.messaging.ReplyFuture;
import com.google.gson.Gson;
import com.google.gson.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import org.eclipse.paho.client.mqttv3.IMqttDeliveryToken;
import org.eclipse.paho.client.mqttv3.MqttCallback;
import org.eclipse.paho.client.mqttv3.MqttCallbackExtended;
import org.eclipse.paho.client.mqttv3.*;
import software.amazon.awssdk.aws.greengrass.model.QOS;
import javax.net.ssl.SSLContext;
import javax.net.ssl.SSLSocketFactory;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.security.KeyStore;
import java.security.PrivateKey;
import java.security.cert.CertificateFactory;
import java.util.UUID;
import java.util.concurrent.*;
import java.util.function.BiConsumer;
import java.io.*;
import java.nio.file.*;
import java.security.cert.*;
import javax.net.ssl.*;


public class StandaloneMessagingProvider extends MessagingProvider
{
    protected static final Logger LOGGER = LogManager.getLogger(StandaloneMessagingProvider.class);


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
        public int qos;
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

        // Catching InterruptedException to ensure queue processing continues even if there is an exception from
        // processing a single message
        @SuppressWarnings("ThreadInterruptedCheck")
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
                    if (responseFutures.containsKey(topic)) {
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

        void shutdown()
        {
            queue.add(new QueueEntry(null, null)); // sentinel to break the processing loop
            executor.shutdownNow();
        }
    }

    private final ConcurrentHashMap<String,SubscriptionProcessor> localSubscriptionProcessors = new ConcurrentHashMap<>();
    private final ConcurrentHashMap<String,SubscriptionProcessor> iotCoreSubscriptionProcessors = new ConcurrentHashMap<>();
    private final MqttClient localMqttClient;
    private final MqttClient iotCoreMqttClient;
    private final ConcurrentHashMap<String, ReplyFuture> responseFutures = new ConcurrentHashMap<>();

    private class EventCallback implements MqttCallbackExtended
    {
        private final MqttClient client;
        private final ConcurrentHashMap<String,SubscriptionProcessor> subscriptionMap;

        public EventCallback(MqttClient client, ConcurrentHashMap<String,SubscriptionProcessor> subscriptionMap)
        {
            this.client = client;
            this.subscriptionMap = subscriptionMap;
        }

        @Override
        public void connectComplete(boolean reconnect, String serverURI)
        {
            if (reconnect)
            {
                LOGGER.info("Reconnected to MQTT broker {}. Re-subscribing to {} topic(s).",
                        serverURI, subscriptionMap.size());
                for (SubscriptionProcessor processor : subscriptionMap.values())
                {
                    try
                    {
                        client.subscribe(processor.topicFilter, processor.qos);
                    }
                    catch (MqttException e)
                    {
                        LOGGER.error("Failed to re-subscribe to '{}' after reconnect: {}",
                                processor.topicFilter, e.toString());
                    }
                }
            }
        }

        @Override
        public void connectionLost(Throwable cause)
        {
            // Automatic reconnect is enabled on the client; re-subscription happens in connectComplete().
            LOGGER.error("Connection to MQTT broker lost - {}. Automatic reconnect in progress.", cause.toString());
        }

        @Override
        public void messageArrived(String topic, MqttMessage message)
        {
            Message msg;
            LOGGER.trace("Message received on topic '{}'", topic);
            String msgChars = new String(message.getPayload(), StandardCharsets.UTF_8);
            try {
                msg = MessageBuilder.fromObject(new Gson().fromJson(msgChars, JsonObject.class));
            } catch (Exception e) {
                msg = MessageBuilder.fromObject(msgChars);
            }
            
            SubscriptionProcessor subscriptionProcessor = subscriptionMap.get(topic);
            if (subscriptionProcessor == null) {
                for (SubscriptionProcessor processor : subscriptionMap.values()) {
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

    public StandaloneMessagingProvider(MessagingConfiguration config, String thingName) {
        try {
            // Initialize local MQTT client
            MessagingConfiguration.LocalMqttConfig localConfig = config.getMessaging().getLocal();
            MessagingConfiguration.IoTCoreConfig iotCoreConfig = config.getMessaging().getIotCore();

            boolean useSSL = localConfig.getCredentials() != null && localConfig.getCredentials().getCaPath() != null;
            String protocol = useSSL ? "ssl" : "tcp";
            String localBrokerUrl = protocol + "://" + localConfig.getHost() + ":" + localConfig.getPort();
            localMqttClient = new MqttClient(localBrokerUrl, localConfig.getClientId());

            MqttConnectOptions localOptions = new MqttConnectOptions();
            localOptions.setAutomaticReconnect(true);
            if (localConfig.getCredentials() != null) {
                if (useSSL) {
                    // TLS: server trust via CA, with optional client-certificate (mutual) auth
                    localOptions.setSocketFactory(createSslContext(localConfig.getCredentials()).getSocketFactory());
                } else if (localConfig.getCredentials().getUsername() != null
                        && localConfig.getCredentials().getPassword() != null) {
                    // Use username/password authentication
                    localOptions.setUserName(localConfig.getCredentials().getUsername());
                    localOptions.setPassword(localConfig.getCredentials().getPassword().toCharArray());
                }
            }
            localMqttClient.setCallback(new EventCallback(localMqttClient, localSubscriptionProcessors));
            localMqttClient.connect(localOptions);
            LOGGER.info("Connected to local broker at {}", localBrokerUrl);

            // Initialize IoT Core MQTT client
            String iotCoreBrokerUrl = "ssl://" + iotCoreConfig.getEndpoint() + ":" + iotCoreConfig.getPort();
            iotCoreMqttClient = new MqttClient(iotCoreBrokerUrl, iotCoreConfig.getClientId());
            connectToIotCore(iotCoreConfig);

        } catch (Exception e) {
            LOGGER.error("Failed to initialize MQTT clients", e);
            throw new RuntimeException("Failed to initialize MQTT clients", e);
        }
    }

    private SSLContext createSslContext(MessagingConfiguration.CredentialsConfig credentials) throws Exception {
        // Load CA certificate (required for TLS — establishes server trust)
        X509Certificate caCert = (X509Certificate) CertificateFactory.getInstance("X.509")
                                                                     .generateCertificate(new ByteArrayInputStream(Files.readAllBytes(Paths.get(credentials.getCaPath()))));

        // Create trust store for the CA certificate
        KeyStore caKs = KeyStore.getInstance(KeyStore.getDefaultType());
        caKs.load(null, null);
        caKs.setCertificateEntry("ca-certificate", caCert);
        TrustManagerFactory tmf = TrustManagerFactory.getInstance(TrustManagerFactory.getDefaultAlgorithm());
        tmf.init(caKs);

        // Client certificate + private key are optional: present => mutual TLS, absent => server-only TLS.
        KeyManager[] keyManagers = null;
        if (credentials.getCertPath() != null && credentials.getKeyPath() != null)
        {
            X509Certificate clientCert = (X509Certificate) CertificateFactory.getInstance("X.509")
                                                                             .generateCertificate(new ByteArrayInputStream(Files.readAllBytes(Paths.get(credentials.getCertPath()))));
            PrivateKey privateKey = PrivateKeyReader.getPrivateKey(credentials.getKeyPath());
            char[] password = java.util.UUID.randomUUID().toString().toCharArray();
            KeyStore ks = KeyStore.getInstance(KeyStore.getDefaultType());
            ks.load(null, null);
            ks.setCertificateEntry("certificate", clientCert);
            ks.setKeyEntry("private-key", privateKey, password, new java.security.cert.Certificate[]{clientCert});
            KeyManagerFactory kmf = KeyManagerFactory.getInstance(KeyManagerFactory.getDefaultAlgorithm());
            kmf.init(ks, password);
            keyManagers = kmf.getKeyManagers();
        }

        // Create SSL context
        SSLContext context = SSLContext.getInstance("TLSv1.2");
        context.init(keyManagers, tmf.getTrustManagers(), null);

        return context;
    }

    public void connectToIotCore(MessagingConfiguration.IoTCoreConfig config)
    {
        String uri = String.format("ssl://%s:%d", config.getEndpoint(), config.getPort());
        try
        {
            iotCoreMqttClient.setCallback(new EventCallback(iotCoreMqttClient, iotCoreSubscriptionProcessors));
            MqttConnectOptions connOpts = new MqttConnectOptions();
            connOpts.setAutomaticReconnect(true);
            SSLSocketFactory socketFactory = getSocketFactory(config.getCredentials().getCaPath(),
                    config.getCredentials().getCertPath(),
                    config.getCredentials().getKeyPath());
            if (socketFactory != null)
            {
                connOpts.setSocketFactory(socketFactory);
                LOGGER.info("Attempting to connect to IoT Core at {}", uri);
                LOGGER.info("       ...using private key file: {}", config.getCredentials().getKeyPath());
                LOGGER.info("       ...using device cert file: {}", config.getCredentials().getCertPath());
                LOGGER.info("       ...using CA Cert file: {}", config.getCredentials().getCaPath());
                LOGGER.info("       ...using client ID: {}", config.getClientId());
            }
            else
            {
                LOGGER.fatal("Unable to load cert/key files for IoT Core at {}. Refusing to connect without credentials.", uri);
                throw new RuntimeException("Unable to load IoT Core credentials (cert/key/CA) for " + uri
                        + "; refusing to connect unauthenticated.");
            }

            connOpts.setCleanSession(true);
            connOpts.setMaxInflight(250);
            connOpts.setConnectionTimeout(10);
            iotCoreMqttClient.connect(connOpts);
            LOGGER.info("Connected to AWS IoT Core broker {}", uri);
        }
        catch (MqttException e)
        {
            LOGGER.fatal("Failed to connect to IoT Core at {} - {}", uri, e.toString());
            throw new RuntimeException("Failed to connect to IoT Core at " + uri, e);
        }
    }

    @Override
    public void close()
    {
        for (SubscriptionProcessor p : localSubscriptionProcessors.values()) { p.shutdown(); }
        for (SubscriptionProcessor p : iotCoreSubscriptionProcessors.values()) { p.shutdown(); }
        localSubscriptionProcessors.clear();
        iotCoreSubscriptionProcessors.clear();
        disconnectQuietly(localMqttClient);
        disconnectQuietly(iotCoreMqttClient);
    }

    private void disconnectQuietly(MqttClient client)
    {
        if (client == null)
        {
            return;
        }
        try
        {
            if (client.isConnected())
            {
                client.disconnect();
            }
            client.close();
        }
        catch (MqttException e)
        {
            LOGGER.warn("Error disconnecting MQTT client: {}", e.toString());
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
            SSLContext context = SSLContext.getInstance("TLSv1.2");
            context.init(kmf.getKeyManagers(), tmf.getTrustManagers(), null);
            retVal = context.getSocketFactory();
        } catch (Exception e) {
            LOGGER.error("Failed to load certificates - {}", e.toString());
        }
        return retVal;
    }

    private void internalPublish(MqttClient client, String topic, Message message, QOS qos) {
        try
        {
            MqttMessage msg = new MqttMessage(message.toString().getBytes());
            msg.setQos(qos.ordinal());
            client.publish(topic, msg);
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
        internalPublish(localMqttClient, topic, message, QOS.AT_LEAST_ONCE);
    }

    @Override
    public void publishToIoTCore(String topic, Message message, QOS qos)
    {
        internalPublish(iotCoreMqttClient, topic, message, qos);
    }

    @Override
    public void publishRaw(String topic, JsonObject payload)
    {
        try
        {
            MqttMessage msg = new MqttMessage(payload.toString().getBytes());
            localMqttClient.publish(topic, msg);
        }
        catch (MqttException e)
        {
            LOGGER.error("Failed to publish raw message on topic '{}' - {}",
                    topic, e.toString());
        }
    }

    @Override
    public void publishToIoTCoreRaw(String topic, JsonObject payload, QOS qos)
    {
        try
        {
            MqttMessage msg = new MqttMessage(payload.toString().getBytes());
            msg.setQos(qos.ordinal());
            iotCoreMqttClient.publish(topic, msg);
        }
        catch (MqttException e)
        {
            LOGGER.error("Failed to publish raw message on topic '{}' - {}",
                    topic, e.toString());
        }
    }

    private void internalSubscribe(MqttClient client, String topicFilter, BiConsumer<String, Message> callback, QOS qos, int maxConcurrency, ConcurrentHashMap<String,SubscriptionProcessor> subscriptionMap)
    {
        SubscriptionProcessor subProcessor = new SubscriptionProcessor(topicFilter, callback, maxConcurrency);
        subProcessor.qos = qos.ordinal();
        subscriptionMap.put(topicFilter, subProcessor);
        try
        {
            client.subscribe(topicFilter, qos.ordinal());
        }
        catch (MqttException e)
        {
            LOGGER.error("Failed to subscribe to topicFilter '{}': {}", topicFilter, e.toString());
        }
    }

    public void subscribe(String topicFilter, BiConsumer<String, Message> callback, int maxConcurrency)
    {
        internalSubscribe(localMqttClient, topicFilter, callback, QOS.AT_LEAST_ONCE, maxConcurrency, localSubscriptionProcessors);
    }

    @Override
    public void subscribeToIoTCore(String topicFilter, BiConsumer<String, Message> callback, QOS qos,
                                   int maxConcurrency)
    {
        internalSubscribe(iotCoreMqttClient, topicFilter, callback, qos, maxConcurrency, iotCoreSubscriptionProcessors);
    }

    private void internalUnsubscribe(MqttClient client, String topicFilter, ConcurrentHashMap<String,SubscriptionProcessor> subscriptionMap)
    {
        try
        {
            SubscriptionProcessor subProcessor = subscriptionMap.get(topicFilter);
            if (subProcessor != null) {
                subProcessor.queue.put(new QueueEntry(null, null));
                subscriptionMap.remove(topicFilter);
                client.unsubscribe(topicFilter);
            }
        }
        catch (Exception e)
        {
            LOGGER.warn("Problem unsubscribing from '{}': {}", topicFilter, e.getMessage());
        }
    }


    @Override
    public void unsubscribe(String topicFilter)
    {
        internalUnsubscribe(localMqttClient, topicFilter, localSubscriptionProcessors);
    }

    @Override
    public void unsubscribeFromIoTCore(String topicFilter)
    {
        internalUnsubscribe(iotCoreMqttClient, topicFilter, iotCoreSubscriptionProcessors);
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
        internalSubscribe(iotCoreMqttClient, replyTo, null, QOS.AT_MOST_ONCE, 1, iotCoreSubscriptionProcessors);
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
    public Object getNativeLocalClient()
    {
        return localMqttClient;
    }

    @Override
    public Object getNativeIotCoreClient() { return iotCoreMqttClient; }
}
