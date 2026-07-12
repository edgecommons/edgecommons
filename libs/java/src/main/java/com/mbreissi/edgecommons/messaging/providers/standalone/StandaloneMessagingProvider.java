/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.messaging.providers.standalone;

import com.mbreissi.edgecommons.messaging.Message;
import com.mbreissi.edgecommons.messaging.MessagingConfiguration;
import com.mbreissi.edgecommons.messaging.MessagingProvider;
import com.mbreissi.edgecommons.messaging.PublishConfirmationException;
import com.mbreissi.edgecommons.messaging.Qos;
import com.mbreissi.edgecommons.messaging.ReplyFuture;
import com.google.gson.Gson;
import com.google.gson.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import org.eclipse.paho.client.mqttv3.IMqttDeliveryToken;
import org.eclipse.paho.client.mqttv3.MqttCallback;
import org.eclipse.paho.client.mqttv3.MqttCallbackExtended;
import org.eclipse.paho.client.mqttv3.*;
import javax.net.ssl.SSLContext;
import javax.net.ssl.SSLSocketFactory;
import java.nio.file.Files;
import java.security.KeyStore;
import java.security.PrivateKey;
import java.security.cert.CertificateFactory;
import java.time.Duration;
import java.util.UUID;
import java.util.concurrent.*;
import java.util.function.BiConsumer;
import java.io.*;
import java.nio.file.*;
import java.security.cert.*;
import javax.net.ssl.*;


public final class StandaloneMessagingProvider extends MessagingProvider
{
    protected static final Logger LOGGER = LogManager.getLogger(StandaloneMessagingProvider.class);

    private final int localPublishQos;
    private final int localSubscribeQos;
    private final int iotCorePublishQos;
    private final int iotCoreSubscribeQos;

    /** Bounds QoS-1 operations waiting for PUBACK; callers experience blocking backpressure. */
    private final Semaphore confirmedPublishPermits = new Semaphore(
            DEFAULT_MAX_IN_FLIGHT_CONFIRMED_PUBLISHES);

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
        public int maxMessages;
        public int qos;
        public LinkedBlockingQueue<QueueEntry> queue;
        ExecutorService executor;
        private final Semaphore concurrencyLimit;
        /** The MQTT client this subscription lives on (local vs IoT Core) — reply-settle cleanup
         *  must unsubscribe on the owning side, never the other one. */
        private final MqttClient owningClient;
        private final ConcurrentHashMap<String, SubscriptionProcessor> owningMap;

        SubscriptionProcessor(MqttClient owningClient,
                              ConcurrentHashMap<String, SubscriptionProcessor> owningMap,
                              String topicFilter, BiConsumer<String, Message> callback,
                              int maxConcurrency, int maxMessages)
        {
            super();
            this.owningClient = owningClient;
            this.owningMap = owningMap;
            this.topicFilter = topicFilter;
            this.callback = callback;
            this.maxConcurrency = maxConcurrency;
            this.maxMessages = maxMessages;
            // Bounded queue (drop on overflow) when maxMessages > 0, else unbounded — parity with
            // the Rust (bounded mpsc) / TS (drop-on-overflow) providers.
            this.queue = maxMessages > 0 ? new LinkedBlockingQueue<>(maxMessages) : new LinkedBlockingQueue<>();
            // One virtual thread per callback (callbacks block on MQTT / IoT Core I/O).
            // A positive maxConcurrency is enforced with a Semaphore, preserving the
            // bounded-concurrency contract without a fixed platform-thread pool.
            executor = Executors.newVirtualThreadPerTaskExecutor();
            concurrencyLimit = maxConcurrency > 0 ? new Semaphore(maxConcurrency) : null;
            new Thread(this, "standalone-sub-" + topicFilter).start();
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
                    final ReplyFuture future = responseFutures.get(topic);
                    if (future != null) {
                        // Reply arrival: race the single idempotent settle path (§5.1) against the
                        // framework deadline and cancelRequest. The winner owns the cleanup
                        // (unsubscribe on the OWNING client + pending-entry removal) and completes
                        // the future; a loser (straggler reply after settle) is dropped at DEBUG.
                        if (future.trySettle()) {
                            internalUnsubscribe(owningClient, topic, owningMap);
                            responseFutures.remove(topic);
                            future.complete(entry.message);
                        } else {
                            LOGGER.debug("Dropping straggler reply on '{}' (request already settled)", topic);
                        }
                    } else if (callback == null) {
                        // A reply-topic subscription whose pending entry is already gone (the
                        // deadline or cancel settled + cleaned up): drop the straggler.
                        LOGGER.debug("Dropping straggler reply on '{}' (no pending request)", topic);
                    } else {
                        if (concurrencyLimit != null)
                        {
                            // Backpressure: at most maxConcurrency callbacks in flight.
                            concurrencyLimit.acquireUninterruptibly();
                        }
                        executor.execute(() -> {
                            try
                            {
                                LOGGER.debug("Invoking callback for topic '{}'", topic);
                                callback.accept(topic, entry.message);
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
            LOGGER.trace("Message received on topic '{}'", topic);
            Message msg;
            try {
                msg = Message.fromBytes(message.getPayload());
            } catch (IllegalArgumentException e) {
                LOGGER.warn("Problem decoding MQTT payload into EdgeCommons protobuf Message on topic '{}': {}. Ignoring message.",
                        topic, e.toString());
                return;
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
                // Non-blocking enqueue: a full bounded queue drops the message with a warning
                // rather than blocking the MQTT callback thread (parity with Rust/TS).
                if (!subscriptionProcessor.queue.offer(new QueueEntry(topic, msg))) {
                    LOGGER.warn("Subscription queue full (maxMessages={}) for '{}'; dropping message on {}",
                            subscriptionProcessor.maxMessages, subscriptionProcessor.topicFilter, topic);
                }
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
            MessagingConfiguration.MessagingConfig messagingConfig = config.getMessaging();
            MessagingConfiguration.LocalMqttConfig localConfig = messagingConfig.getLocal();
            MessagingConfiguration.NorthboundMqttConfig northboundConfig = messagingConfig.getNorthbound();
            localPublishQos = validateQos(
                    localConfig.getQos() == null ? 1 : localConfig.getQos().publishOrDefault(),
                    2,
                    "messaging.local.qos.publish");
            localSubscribeQos = validateQos(
                    localConfig.getQos() == null ? 1 : localConfig.getQos().subscribeOrDefault(),
                    2,
                    "messaging.local.qos.subscribe");
            iotCorePublishQos = validateQos(
                    northboundConfig == null || northboundConfig.getQos() == null ? 1 : northboundConfig.getQos().publishOrDefault(),
                    2,
                    "messaging.northbound.qos.publish");
            iotCoreSubscribeQos = validateQos(
                    northboundConfig == null || northboundConfig.getQos() == null ? 1 : northboundConfig.getQos().subscribeOrDefault(),
                    2,
                    "messaging.northbound.qos.subscribe");

            boolean useSSL = localConfig.getCredentials() != null && localConfig.getCredentials().getCaPath() != null;
            String protocol = useSSL ? "ssl" : "tcp";
            String localBrokerUrl = protocol + "://" + localConfig.getHost() + ":" + localConfig.getPort();
            localMqttClient = new MqttClient(localBrokerUrl, localConfig.getClientId());

            MqttConnectOptions localOptions = buildLocalConnectOptions(localConfig);
            localMqttClient.setCallback(new EventCallback(localMqttClient, localSubscriptionProcessors));
            localMqttClient.connect(localOptions);
            LOGGER.info("Connected to local broker at {}", localBrokerUrl);

            // Initialize northbound MQTT client (optional — only when a northbound section is present)
            if (northboundConfig != null) {
                boolean northboundUseSSL = northboundConfig.getCredentials() != null
                        && northboundConfig.getCredentials().getCaPath() != null;
                String northboundProtocol = northboundUseSSL ? "ssl" : "tcp";
                String northboundBrokerUrl = northboundProtocol + "://"
                        + northboundConfig.getResolvedHost() + ":" + northboundConfig.getPort();
                iotCoreMqttClient = new MqttClient(northboundBrokerUrl, northboundConfig.getClientId());
                connectToNorthbound(northboundConfig, northboundBrokerUrl);
            } else {
                iotCoreMqttClient = null;
                LOGGER.info("No 'northbound' section in the standalone messaging config; northbound messaging is disabled.");
            }

        } catch (Exception e) {
            LOGGER.error("Failed to initialize MQTT clients", e);
            throw new RuntimeException("Failed to initialize MQTT clients", e);
        }
    }

    private static int validateQos(int qos, int max, String field) {
        if (qos < 0 || qos > max) {
            throw new IllegalArgumentException(field + " must be 0.." + max + " (got " + qos + ")");
        }
        return qos;
    }

    /**
     * Builds the connect options for the <em>local-broker</em> connection: automatic reconnect
     * plus TLS / username-password credential wiring. Package-private as the connect-options test
     * seam: tests assert the produced options directly, without a broker.
     */
    static MqttConnectOptions buildLocalConnectOptions(MessagingConfiguration.LocalMqttConfig localConfig) throws Exception {
        boolean useSSL = localConfig.getCredentials() != null && localConfig.getCredentials().getCaPath() != null;
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
        return localOptions;
    }

    static SSLContext createSslContext(MessagingConfiguration.CredentialsConfig credentials) throws Exception {
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

    public void connectToNorthbound(MessagingConfiguration.NorthboundMqttConfig config, String uri)
    {
        try
        {
            iotCoreMqttClient.setCallback(new EventCallback(iotCoreMqttClient, iotCoreSubscriptionProcessors));
            MqttConnectOptions connOpts = new MqttConnectOptions();
            connOpts.setAutomaticReconnect(true);
            MessagingConfiguration.CredentialsConfig credentials = config.getCredentials();
            if (credentials != null) {
                if (credentials.getCaPath() != null) {
                    connOpts.setSocketFactory(createSslContext(credentials).getSocketFactory());
                    LOGGER.info("Attempting to connect to northbound MQTT broker over TLS at {}", uri);
                } else if (credentials.getUsername() != null && credentials.getPassword() != null) {
                    connOpts.setUserName(credentials.getUsername());
                    connOpts.setPassword(credentials.getPassword().toCharArray());
                    LOGGER.info("Attempting to connect to northbound MQTT broker with username/password at {}", uri);
                } else {
                    LOGGER.info("Attempting to connect to northbound MQTT broker without credentials at {}", uri);
                }
            } else {
                LOGGER.info("Attempting to connect to northbound MQTT broker without credentials at {}", uri);
            }

            connOpts.setCleanSession(true);
            connOpts.setMaxInflight(250);
            connOpts.setConnectionTimeout(10);
            iotCoreMqttClient.connect(connOpts);
            LOGGER.info("Connected to northbound MQTT broker {}", uri);
        }
        catch (Exception e)
        {
            LOGGER.fatal("Failed to connect to northbound MQTT broker at {} - {}", uri, e.toString());
            throw new RuntimeException("Failed to connect to northbound MQTT broker at " + uri, e);
        }
    }

    @Override
    public void close()
    {
        super.close();  // shuts down the shared request-deadline scheduler
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
        }
        catch (MqttException e)
        {
            LOGGER.warn("Error disconnecting MQTT client: {}", e.toString());
        }
        finally
        {
            // Always close, even if disconnect() threw: close() releases the Paho file-persistence
            // lock and tears down the automaticReconnect thread. Skipping it leaks a lingering
            // client that can reconnect to a later test's broker on a reused port.
            try
            {
                client.close();
            }
            catch (MqttException e)
            {
                LOGGER.warn("Error closing MQTT client: {}", e.toString());
            }
        }
    }

    static SSLSocketFactory getSocketFactory(final String caCrtFile, final String crtFile, final String keyFile)
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

    private void internalPublish(MqttClient client, String topic, Message message, int qos) {
        try
        {
            MqttMessage msg = new MqttMessage(message.toBytes());
            msg.setQos(qos);
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
        internalPublish(localMqttClient, topic, message, localPublishQos);
    }

    private MqttClient requireIotCore()
    {
        if (iotCoreMqttClient == null)
        {
            throw new IllegalStateException(
                    "Northbound MQTT is not configured in the standalone messaging config (no 'northbound' section)");
        }
        return iotCoreMqttClient;
    }

    @Override
    public void publishNorthbound(String topic, Message message, Qos qos)
    {
        internalPublish(requireIotCore(), topic, message, qos.mqttLevel());
    }

    @Override
    public void publishConfirmed(String topic, byte[] encodedMessage, Qos qos, Duration timeout)
    {
        internalPublishConfirmed(localMqttClient, topic, encodedMessage, qos, timeout);
    }

    @Override
    public void publishNorthboundConfirmed(String topic, byte[] encodedMessage, Qos qos,
                                            Duration timeout)
    {
        internalPublishConfirmed(requireIotCore(), topic, encodedMessage, qos, timeout);
    }

    /**
     * Paho's {@link MqttTopic#publish(MqttMessage)} returns the delivery token whose QoS-1
     * completion is the matching broker PUBACK. Waiting on that token is the strict confirmation
     * boundary; enqueueing through the ordinary client publish method is not enough.
     */
    private void internalPublishConfirmed(MqttClient client, String topic, byte[] encodedMessage,
                                          Qos qos, Duration timeout)
    {
        long timeoutMillis = confirmedTimeoutMillis(encodedMessage, qos, timeout);
        long timeoutNanos = TimeUnit.MILLISECONDS.toNanos(timeoutMillis);
        long started = System.nanoTime();
        boolean acquired = false;
        try
        {
            acquired = confirmedPublishPermits.tryAcquire(timeoutNanos, TimeUnit.NANOSECONDS);
            if (!acquired)
            {
                throw confirmationFailure(PublishConfirmationException.Reason.TIMEOUT, topic,
                        "timed out waiting for confirmed-publish capacity", null);
            }

            long elapsed = System.nanoTime() - started;
            long remainingNanos = timeoutNanos - elapsed;
            if (remainingNanos <= 0)
            {
                throw confirmationFailure(PublishConfirmationException.Reason.TIMEOUT, topic,
                        "confirmed-publish timeout elapsed before send", null);
            }

            MqttMessage mqttMessage = new MqttMessage(encodedMessage.clone());
            mqttMessage.setQos(Qos.AT_LEAST_ONCE.mqttLevel());
            MqttDeliveryToken token = client.getTopic(topic).publish(mqttMessage);
            long waitMillis = Math.max(1L,
                    TimeUnit.NANOSECONDS.toMillis(remainingNanos - 1L) + 1L);
            token.waitForCompletion(waitMillis);
            if (!token.isComplete())
            {
                throw confirmationFailure(PublishConfirmationException.Reason.TIMEOUT, topic,
                        "broker PUBACK was not observed before timeout", null);
            }
            if (token.getException() != null)
            {
                throw confirmationFailure(PublishConfirmationException.Reason.TRANSPORT_ERROR,
                        topic, "broker did not acknowledge the publish", token.getException());
            }
        }
        catch (InterruptedException e)
        {
            Thread.currentThread().interrupt();
            throw confirmationFailure(PublishConfirmationException.Reason.INTERRUPTED, topic,
                    "interrupted while waiting for confirmed-publish capacity", e);
        }
        catch (MqttException e)
        {
            PublishConfirmationException.Reason reason =
                    e.getReasonCode() == MqttException.REASON_CODE_CLIENT_TIMEOUT
                            ? PublishConfirmationException.Reason.TIMEOUT
                            : PublishConfirmationException.Reason.TRANSPORT_ERROR;
            throw confirmationFailure(reason, topic,
                    reason == PublishConfirmationException.Reason.TIMEOUT
                            ? "broker PUBACK was not observed before timeout"
                            : "MQTT publish failed before broker acknowledgement", e);
        }
        finally
        {
            if (acquired)
            {
                confirmedPublishPermits.release();
            }
        }
    }

    private static PublishConfirmationException confirmationFailure(
            PublishConfirmationException.Reason reason, String topic, String detail,
            Throwable cause)
    {
        return new PublishConfirmationException(reason,
                "confirmed publish on '" + topic + "' failed: " + detail, cause);
    }

    @Override
    public void publishRaw(String topic, JsonObject payload)
    {
        try
        {
            MqttMessage msg = new MqttMessage(payload.toString().getBytes());
            msg.setQos(localPublishQos);
            localMqttClient.publish(topic, msg);
        }
        catch (MqttException e)
        {
            LOGGER.error("Failed to publish raw message on topic '{}' - {}",
                    topic, e.toString());
        }
    }

    @Override
    public void publishNorthboundRaw(String topic, JsonObject payload, Qos qos)
    {
        try
        {
            MqttMessage msg = new MqttMessage(payload.toString().getBytes());
            msg.setQos(qos.mqttLevel());
            requireIotCore().publish(topic, msg);
        }
        catch (MqttException e)
        {
            LOGGER.error("Failed to publish raw message on topic '{}' - {}",
                    topic, e.toString());
        }
    }

    private void internalSubscribe(MqttClient client, String topicFilter, BiConsumer<String, Message> callback, int qos, int maxConcurrency, int maxMessages, ConcurrentHashMap<String,SubscriptionProcessor> subscriptionMap)
    {
        SubscriptionProcessor subProcessor = new SubscriptionProcessor(client, subscriptionMap, topicFilter, callback, maxConcurrency, maxMessages);
        subProcessor.qos = qos;
        subscriptionMap.put(topicFilter, subProcessor);
        try
        {
            client.subscribe(topicFilter, qos);
        }
        catch (MqttException e)
        {
            subscriptionMap.remove(topicFilter, subProcessor);
            subProcessor.shutdown();
            LOGGER.error("Failed to subscribe to topicFilter '{}': {}", topicFilter, e.toString());
        }
    }

    public void subscribe(String topicFilter, BiConsumer<String, Message> callback, int maxConcurrency, int maxMessages)
    {
        internalSubscribe(localMqttClient, topicFilter, callback, localSubscribeQos, maxConcurrency, maxMessages, localSubscriptionProcessors);
    }

    @Override
    public void subscribeAcknowledged(String topicFilter,
                                      BiConsumer<String, Message> callback,
                                      int maxConcurrency,
                                      int maxMessages,
                                      Duration timeout)
    {
        long timeoutMillis = subscriptionTimeoutMillis(timeout);
        SubscriptionProcessor processor = new SubscriptionProcessor(
                localMqttClient, localSubscriptionProcessors, topicFilter, callback,
                maxConcurrency, maxMessages);
        processor.qos = localSubscribeQos;
        SubscriptionProcessor existing = localSubscriptionProcessors.putIfAbsent(
                topicFilter, processor);
        if (existing != null)
        {
            processor.shutdown();
            throw new IllegalStateException(
                    "a local subscription already exists for '" + topicFilter + "'");
        }

        java.util.concurrent.atomic.AtomicBoolean abandoned =
                new java.util.concurrent.atomic.AtomicBoolean(false);
        ExecutorService waiter = Executors.newVirtualThreadPerTaskExecutor();
        Future<?> operation = waiter.submit(() -> {
            localMqttClient.subscribe(topicFilter, localSubscribeQos);
            if (abandoned.get())
            {
                abandonAcknowledgedSubscription(topicFilter, processor);
            }
            return null;
        });
        try
        {
            operation.get(timeoutMillis, TimeUnit.MILLISECONDS);
        }
        catch (TimeoutException e)
        {
            abandoned.set(true);
            operation.cancel(true);
            abandonAcknowledgedSubscription(topicFilter, processor);
            throw new IllegalStateException(
                    "MQTT SUBACK was not observed before the subscription deadline", e);
        }
        catch (InterruptedException e)
        {
            abandoned.set(true);
            operation.cancel(true);
            abandonAcknowledgedSubscription(topicFilter, processor);
            Thread.currentThread().interrupt();
            throw new IllegalStateException(
                    "interrupted while waiting for MQTT SUBACK", e);
        }
        catch (ExecutionException e)
        {
            abandoned.set(true);
            abandonAcknowledgedSubscription(topicFilter, processor);
            Throwable cause = e.getCause() == null ? e : e.getCause();
            throw new IllegalStateException("MQTT subscription failed before SUBACK", cause);
        }
        finally
        {
            waiter.shutdownNow();
        }
    }

    private void abandonAcknowledgedSubscription(
            String topicFilter, SubscriptionProcessor processor)
    {
        if (localSubscriptionProcessors.remove(topicFilter, processor))
        {
            processor.shutdown();
        }
        Thread.startVirtualThread(() -> {
            try
            {
                if (localMqttClient.isConnected())
                {
                    localMqttClient.unsubscribe(topicFilter);
                }
            }
            catch (Exception e)
            {
                LOGGER.debug("Best-effort cleanup of late subscription '{}' failed: {}",
                        topicFilter, e.toString());
            }
        });
    }

    @Override
    public void subscribeNorthbound(String topicFilter, BiConsumer<String, Message> callback, Qos qos,
                                   int maxConcurrency, int maxMessages)
    {
        internalSubscribe(requireIotCore(), topicFilter, callback, qos.mqttLevel(), maxConcurrency, maxMessages, iotCoreSubscriptionProcessors);
    }

    private void internalUnsubscribe(MqttClient client, String topicFilter, ConcurrentHashMap<String,SubscriptionProcessor> subscriptionMap)
    {
        try
        {
            SubscriptionProcessor subProcessor = subscriptionMap.get(topicFilter);
            if (subProcessor != null) {
                subscriptionMap.remove(topicFilter, subProcessor);
                subProcessor.shutdown();
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
    public void unsubscribeNorthbound(String topicFilter)
    {
        internalUnsubscribe(requireIotCore(), topicFilter, iotCoreSubscriptionProcessors);
    }

    @Override
    public ReplyFuture request(String topic, Message message)
    {
        return request(topic, message, null);
    }

    @Override
    public ReplyFuture request(String topic, Message message, Duration timeout)
    {
        String replyTo = message.makeRequest();
        ReplyFuture future = new ReplyFuture(replyTo);
        responseFutures.put(replyTo, future);
        subscribe(replyTo, null, 1, -1); // one-shot reply sub: unbounded is fine
        // Arm the framework-owned deadline at send time (§5): on expiry the timer unsubscribes the
        // ephemeral reply topic, removes the pending entry and completes the future exceptionally
        // (TimeoutException) — even when the caller never awaits the future.
        armRequestDeadline(future, effectiveRequestTimeout(timeout), () -> {
            unsubscribe(replyTo);
            responseFutures.remove(replyTo);
        });
        publish(topic, message);
        return future;
    }

    @Override
    public void cancelRequest(ReplyFuture future)
    {
        if (!future.trySettle())
        {
            return;  // reply or deadline already settled + cleaned up this request
        }
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
    public ReplyFuture requestNorthbound(String topic, Message message)
    {
        return requestNorthbound(topic, message, null);
    }

    @Override
    public ReplyFuture requestNorthbound(String topic, Message message, Duration timeout)
    {
        String replyTo = message.makeRequest();
        ReplyFuture future = new ReplyFuture(replyTo);
        responseFutures.put(replyTo, future);
        internalSubscribe(requireIotCore(), replyTo, null, iotCoreSubscribeQos, 1, -1, iotCoreSubscriptionProcessors);
        armRequestDeadline(future, effectiveRequestTimeout(timeout), () -> {
            unsubscribeNorthbound(replyTo);
            responseFutures.remove(replyTo);
        });
        internalPublish(requireIotCore(), topic, message, iotCorePublishQos);
        return future;
    }

    @Override
    public void cancelRequestNorthbound(ReplyFuture future)
    {
        if (!future.trySettle())
        {
            return;  // reply or deadline already settled + cleaned up this request
        }
        unsubscribeNorthbound(future.replyTopic);
        responseFutures.remove(future.replyTopic);
        future.complete(null);
    }

    @Override
    public void replyNorthbound(Message request, Message reply)
    {
        reply.setCorrelationId(request.getHeader().getCorrelationId());
        internalPublish(requireIotCore(), request.getHeader().getReplyTo(), reply, iotCorePublishQos);
    }

    // --- test seams (package-private): observe subscription / pending-request state ------------

    /** Whether a local-broker subscription is currently registered for this filter (test seam). */
    boolean hasLocalSubscription(String topicFilter)
    {
        return localSubscriptionProcessors.containsKey(topicFilter);
    }

    /** Whether an IoT Core subscription is currently registered for this filter (test seam). */
    boolean hasIotCoreSubscription(String topicFilter)
    {
        return iotCoreSubscriptionProcessors.containsKey(topicFilter);
    }

    /** Whether a request is still pending on this reply topic (test seam). */
    boolean hasPendingRequest(String replyTopic)
    {
        return responseFutures.containsKey(replyTopic);
    }

    @Override
    public Object getNativeLocalClient()
    {
        return localMqttClient;
    }

    @Override
    public Object getNativeNorthboundClient() { return iotCoreMqttClient; }

    /**
     * Reports the <em>local</em> broker connection state (Paho {@link MqttClient#isConnected()}) — the
     * edge-critical half of the dual-MQTT transport. The IoT Core link is deliberately excluded: a
     * dropped cloud link must not flip readiness while local pub/sub keeps serving. Feeds the
     * readiness model (FR-HB-2).
     *
     * @return {@code true} when the local MQTT client is connected
     */
    @Override
    public boolean connected()
    {
        return localMqttClient != null && localMqttClient.isConnected();
    }
}
