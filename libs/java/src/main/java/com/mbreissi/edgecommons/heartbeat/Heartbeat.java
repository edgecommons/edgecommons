/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.heartbeat;

import com.mbreissi.edgecommons.config.ConfigManager;
import com.mbreissi.edgecommons.config.ConfigurationChangeListener;
import com.mbreissi.edgecommons.config.HeartbeatConfiguration;
import com.mbreissi.edgecommons.messaging.Message;
import com.mbreissi.edgecommons.messaging.MessageBuilder;
import com.mbreissi.edgecommons.messaging.MessageIdentity;
import com.mbreissi.edgecommons.messaging.MessagingClient;
import com.mbreissi.edgecommons.messaging.ReservedPublisher;
import com.mbreissi.edgecommons.metrics.Metric;
import com.mbreissi.edgecommons.metrics.MetricBuilder;
import com.mbreissi.edgecommons.metrics.MetricEmitter;
import com.mbreissi.edgecommons.uns.Uns;
import com.mbreissi.edgecommons.uns.UnsClass;
import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import software.amazon.awssdk.aws.greengrass.model.QOS;

import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;

/**
 * The component heartbeat (UNS-CANONICAL-DESIGN §4.3, D-U14/D-U20). Each tick it publishes a UNS
 * {@code state} keepalive to {@code ecv1/{device}/{component}/main/state} — header name
 * {@value #STATE_MESSAGE_NAME}, body {@code {"status":"RUNNING","uptimeSecs":<n>}} — through the
 * privileged {@link ReservedPublisher} seam (the {@code state} class is reserved), and emits the
 * enabled system measures (cpu/memory/disk/…) as a metric named {@value #SYS_METRIC_NAME} through
 * the normal metric subsystem (D6 — the measures keep the metric subsystem's full sink routing).
 * On graceful shutdown ({@link #close()}) a best-effort {@code {"status":"STOPPED"}} state is
 * published. {@code heartbeat.destination} ({@code local}|{@code iotcore}) selects the keepalive's
 * transport only. Defaults: on / 5 s / local (M11).
 */
public class Heartbeat implements ConfigurationChangeListener
{
    protected static final Logger LOGGER = LogManager.getLogger(Heartbeat.class);

    /** The state keepalive's envelope header name (§4.3). */
    static final String STATE_MESSAGE_NAME = "state";
    static final String STATE_MESSAGE_VERSION = "1.0";
    /** The metric the heartbeat measures are emitted as (§4.3, D-U20/D6). */
    static final String SYS_METRIC_NAME = "sys";

    private final ConfigManager configurationService;
    private MessagingClient messagingService;
    private MetricEmitter metricService;
    private HeartbeatMonitor heartbeatMonitor;
    /** Monotonic start reference for the keepalive's {@code uptimeSecs}. */
    private final long startNanos = System.nanoTime();
    /** Ensures the best-effort STOPPED state is published at most once. */
    private volatile boolean stoppedPublished = false;
    /** WARN-once flag for the no-resolved-identity (test/subclass bring-up) case. */
    private volatile boolean warnedNoIdentity = false;
    /**
     * An optional component-supplied source of per-instance connectivity, sampled each keepalive
     * tick into the state body's {@code instances[]} array (see {@link InstanceConnectivityProvider}).
     */
    private volatile InstanceConnectivityProvider connectivityProvider;
    private final ScheduledExecutorService scheduler =
            Executors.newSingleThreadScheduledExecutor(runnable -> {
                Thread thread = new Thread(runnable, "Heartbeat-scheduler");
                thread.setDaemon(true);
                return thread;
            });
    private ScheduledFuture<?> heartbeatTask;
    private final Object timerLock = new Object();

    /**
     * Package-private constructor used by HeartbeatBuilder.
     * Use HeartbeatBuilder.create() instead of calling this directly.
     */
    Heartbeat(ConfigManager configurationService, MessagingClient messagingService, MetricEmitter metricService)
    {
        this.configurationService = configurationService;
        this.messagingService = messagingService;
        this.metricService = metricService;

        configurationService.addConfigChangeListener(this);
        initialize();
    }

    /**
     * Registers (or replaces, or clears with {@code null}) the per-instance connectivity provider
     * whose result is emitted in each RUNNING {@code state} keepalive's {@code instances[]} array —
     * the overridable surface a multi-connection component uses to report connectivity at the
     * instance level without a separate UNS instance per connection. Wired from
     * {@code EdgeCommons.setInstanceConnectivityProvider}. Thread-safe (volatile).
     *
     * @param provider the provider, or {@code null} to stop reporting per-instance connectivity
     */
    public void setInstanceConnectivityProvider(InstanceConnectivityProvider provider)
    {
        this.connectivityProvider = provider;
    }

    /**
     * Initializes the heartbeat after all dependencies are set.
     */
    private void initialize() {
        defineMetric();
        initHeartbeat();
    }

    /**
     * (Re)initializes the heartbeat from the current configuration: cancels any running task and,
     * when {@code heartbeat.enabled} (the default), schedules the periodic tick at
     * {@code heartbeat.intervalSecs}.
     */
    private void initHeartbeat()
    {
        synchronized (timerLock) {
            // Reschedule on the same executor: cancel the current task and submit a
            // new one at the configured interval. The task is a reusable Runnable, so
            // the executor is kept alive across reconfigures and only shut down in close().
            if (heartbeatTask != null) {
                heartbeatTask.cancel(false);
                heartbeatTask = null;
            }
            HeartbeatConfiguration config = configurationService.getHeartbeatConfig();
            if (!config.isEnabled()) {
                LOGGER.info("Heartbeat disabled by configuration (heartbeat.enabled=false)");
                return;
            }
            heartbeatMonitor = new HeartbeatMonitor(config);
            long periodMs = config.getIntervalSecs() * 1000L;
            heartbeatTask = scheduler.scheduleAtFixedRate(this::runHeartbeat, 0, periodMs, TimeUnit.MILLISECONDS);
            LOGGER.info("Heartbeat initialized at {} second interval (state keepalive -> {})",
                    config.getIntervalSecs(), config.getDestination());
        }
    }

    /**
     * Defines the {@value #SYS_METRIC_NAME} metric (the heartbeat measures) in the metric
     * subsystem.
     */
    private void defineMetric()
    {
        int storageResolution = configurationService.getHeartbeatConfig().getIntervalSecs() < 60 ? 1 : 60;
        Metric metric = MetricBuilder.create(SYS_METRIC_NAME)
                .withNamespace(configurationService.getMetricConfig().getNamespace())
                .withConfig(configurationService)
                .addMeasure("disk_total", "Gigabytes", storageResolution)
                .addMeasure("disk_used", "Gigabytes", storageResolution)
                .addMeasure("disk_free", "Gigabytes", storageResolution)
                .addMeasure("cpu_usage", "Percent", storageResolution)
                .addMeasure("memory_usage", "Megabytes", storageResolution)
                .addMeasure("threads", "Count", storageResolution)
                .addMeasure("files", "Count", storageResolution)
                .addMeasure("fds", "Count", storageResolution)
                .build();
        metricService.defineMetric(metric);
    }

    /**
     * One heartbeat tick (§4.3): the {@code state} keepalive
     * ({@code {"status":"RUNNING","uptimeSecs":n}}) plus the measures as the
     * {@value #SYS_METRIC_NAME} metric. Each half is best-effort — a failure in one must not
     * suppress the other.
     */
    private void publishHeartbeat()
    {
        try
        {
            publishState("RUNNING", true);
        }
        catch (Exception e)
        {
            LOGGER.warn("Heartbeat state keepalive failed: {}", e.toString());
        }
        try
        {
            emitSysMetric();
        }
        catch (Exception e)
        {
            LOGGER.warn("Heartbeat '{}' metric emit failed: {}", SYS_METRIC_NAME, e.toString());
        }
    }

    /**
     * Publishes one {@code state} envelope to the component's UNS state topic through the
     * privileged seam. No-op (WARN once) when the component identity is not resolved (mock/test
     * bring-up — a real {@code ConfigManager} always resolves one).
     *
     * @param status        {@code "RUNNING"} or {@code "STOPPED"}
     * @param includeUptime whether the body carries {@code uptimeSecs} (the RUNNING keepalive)
     */
    private void publishState(String status, boolean includeUptime)
    {
        MessageIdentity identity = configurationService.getComponentIdentity();
        if (identity == null)
        {
            if (!warnedNoIdentity)
            {
                warnedNoIdentity = true;
                LOGGER.warn("No resolved component identity - the heartbeat state keepalive is disabled");
            }
            return;
        }
        String topic = new Uns(identity, configurationService.isTopicIncludeRoot())
                .topic(UnsClass.STATE);

        JsonObject body = new JsonObject();
        body.addProperty("status", status);
        if (includeUptime)
        {
            body.addProperty("uptimeSecs", getUptimeSecs());
        }
        // Per-instance connectivity — the state body's instances[] (only on the RUNNING keepalive; a
        // STOPPED state carries no live instances). Best-effort: a null/throwing provider simply omits
        // the section — a provider bug must never suppress the keepalive itself.
        InstanceConnectivityProvider provider = connectivityProvider;
        if (includeUptime && provider != null)
        {
            try
            {
                List<InstanceConnectivity> conns = provider.instanceConnectivity();
                if (conns != null && !conns.isEmpty())
                {
                    JsonArray instances = new JsonArray();
                    for (InstanceConnectivity c : conns)
                    {
                        if (c != null)
                        {
                            instances.add(c.toJson());
                        }
                    }
                    if (!instances.isEmpty())
                    {
                        body.add("instances", instances);
                    }
                }
            }
            catch (Exception e)
            {
                LOGGER.warn("Instance connectivity provider failed; omitting instances[] this tick: {}",
                        e.toString());
            }
        }
        Message stateMessage = MessageBuilder.create(STATE_MESSAGE_NAME, STATE_MESSAGE_VERSION)
                .withPayload(body)
                .withConfig(configurationService)
                .build();

        ReservedPublisher publisher = messagingService.reservedPublisher();
        String destination = configurationService.getHeartbeatConfig().getDestination();
        if (destination != null && (destination.equalsIgnoreCase("iotcore")
                || destination.equalsIgnoreCase("iot_core")))
        {
            publisher.publishToIoTCore(topic, stateMessage, QOS.AT_LEAST_ONCE);
        }
        else
        {
            publisher.publish(topic, stateMessage);
        }
    }

    /**
     * Re-emits the RUNNING {@code state} keepalive immediately, out of band from the periodic
     * schedule — the {@code republish-state} broadcast re-announce (DESIGN-uns §9.3/§9.4, the
     * late-join lever): same payload as a tick's keepalive
     * ({@code {"status":"RUNNING","uptimeSecs":n}}), same {@link ReservedPublisher} seam, same
     * {@code heartbeat.destination} routing. Respects {@code heartbeat.enabled}: a component
     * whose operator disabled the state keepalive does not re-announce state (the broadcast
     * cannot re-enable an opted-out state surface). Best-effort — failures are logged and
     * swallowed; the periodic schedule is unaffected.
     */
    public void publishStateNow()
    {
        if (!configurationService.getHeartbeatConfig().isEnabled())
        {
            LOGGER.debug("republish-state re-announce skipped: the heartbeat state keepalive is"
                    + " disabled (heartbeat.enabled=false)");
            return;
        }
        try
        {
            publishState("RUNNING", true);
        }
        catch (Exception e)
        {
            LOGGER.warn("Out-of-band state re-announce failed: {}", e.toString());
        }
    }

    /**
     * The component's monotonic uptime in whole seconds — the same value the RUNNING {@code state}
     * keepalive carries as {@code uptimeSecs}. Consumed by the command inbox's {@code ping}
     * built-in verb (DESIGN-uns §9.5) so ping replies and keepalives agree on one uptime source.
     *
     * @return seconds since this heartbeat (i.e. the runtime) was constructed
     */
    public long getUptimeSecs()
    {
        return (System.nanoTime() - startNanos) / 1_000_000_000L;
    }

    /**
     * Emits the enabled measures as the {@value #SYS_METRIC_NAME} metric through the normal
     * metric subsystem (its configured target: messaging/cloudwatch/EMF/log/prometheus).
     */
    private void emitSysMetric()
    {
        JsonObject data = heartbeatMonitor.getStats();
        var measureValues = new HashMap<String, Float>();
        for (Map.Entry<String, JsonElement> entry : data.entrySet())
        {
            for (String measureName : entry.getValue().getAsJsonObject().keySet())
            {
                measureValues.put(measureName, entry.getValue().getAsJsonObject().get(measureName).getAsFloat());
            }
        }
        metricService.emitMetricNow(SYS_METRIC_NAME, measureValues);
    }

    /**
     * Stops the heartbeat: cancels the periodic timer and publishes the best-effort
     * {@code {"status":"STOPPED"}} state (§4.3/D-U14 — at most once; failures are swallowed, the
     * shutdown must proceed).
     */
    public void close()
    {
        boolean wasRunning;
        synchronized (timerLock)
        {
            wasRunning = heartbeatTask != null;
            if (heartbeatTask != null)
            {
                heartbeatTask.cancel(false);
                heartbeatTask = null;
            }
            scheduler.shutdownNow();
        }
        if (wasRunning && !stoppedPublished)
        {
            stoppedPublished = true;
            try
            {
                publishState("STOPPED", false);
            }
            catch (Exception e)
            {
                LOGGER.debug("Best-effort STOPPED state publish failed: {}", e.toString());
            }
        }
    }

    @Override
    /**
     * Handles configuration changes by reinitializing the heartbeat mechanism.
     *
     * @return true if the configuration change was handled successfully
     */
    public boolean onConfigurationChanged()
    {
        LOGGER.info("Configuration changed, restarting heartbeat");
        initHeartbeat();
        return true;
    }

    /**
     * The periodic heartbeat task. Guarded so an exception in one run cannot
     * propagate to the scheduler and silently cancel future heartbeats.
     */
    private void runHeartbeat()
    {
        try
        {
            publishHeartbeat();
        }
        catch (Exception e)
        {
            LOGGER.error("Heartbeat task failed; will retry next interval", e);
        }
    }
}
