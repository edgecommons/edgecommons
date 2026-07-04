/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons;

import com.mbreissi.ggcommons.config.ConfigManager;
import com.mbreissi.ggcommons.facades.AppFacade;
import com.mbreissi.ggcommons.facades.DataFacade;
import com.mbreissi.ggcommons.facades.EventsFacade;
import com.mbreissi.ggcommons.facades.StreamSink;
import com.mbreissi.ggcommons.messaging.MessageBuilder;
import com.mbreissi.ggcommons.messaging.MessageIdentity;
import com.mbreissi.ggcommons.messaging.MessagingClient;
import com.mbreissi.ggcommons.uns.Uns;

import java.time.Clock;

/**
 * The per-instance seam (UNS-CANONICAL-DESIGN §3, D-U3): an instance-scoped handle whose only
 * job is to pre-bind the instance token into (a) the {@link Uns} topic builder, (b) the
 * {@link MessageBuilder}, and (c) the app-usable publish facades ({@code data()}/{@code events()}/
 * {@code app()} — DESIGN-class-facades §3). The messaging client stays instance-agnostic —
 * {@code publish(topic, msg)} already receives both the topic (minted by this handle's
 * instance-bound {@code uns()}) and the envelope (stamped by its instance-bound builder).
 *
 * <p>Obtain handles from {@link GGCommons#instance(String)} (validated + cached per id).
 * Component-level messages (everything not built through a handle) default to instance
 * {@value MessageIdentity#DEFAULT_INSTANCE}.
 */
public final class GgInstance {

    private final String id;
    private final ConfigManager configManager;
    private final Uns uns;
    private final MessagingClient messagingClient;
    private final StreamSink streamSink; // nullable -> data() stream route falls back to local
    private final Clock clock;

    /** Lazily-created facades (per-instance; the facades hold no per-instance client state). */
    private volatile DataFacade data;
    private volatile EventsFacade events;
    private volatile AppFacade app;

    /**
     * Package-private: created by {@link GGCommons#instance(String)}, which validates the token
     * (§2.2 token rule) and caches per id.
     *
     * @param id              the instance token
     * @param configManager   the component config manager
     * @param includeRoot     the resolved {@code topic.includeRoot} mode
     * @param messagingClient the (guarded) messaging client the facades publish through
     * @param streamSink      the stream seam for {@code data().via(stream)}, or {@code null} when
     *                        streaming is not configured (a stream route then falls back to local)
     * @param clock           the clock for the facades' time defaults (injected for deterministic tests)
     */
    GgInstance(String id, ConfigManager configManager, boolean includeRoot,
               MessagingClient messagingClient, StreamSink streamSink, Clock clock) {
        this.id = id;
        this.configManager = configManager;
        this.uns = new Uns(configManager.getComponentIdentity().withInstance(id), includeRoot);
        this.messagingClient = messagingClient;
        this.streamSink = streamSink;
        this.clock = clock;
    }

    /** Returns this handle's instance token. */
    public String id() {
        return id;
    }

    /** Returns the topic builder bound to this instance (topics minted with this instance token). */
    public Uns uns() {
        return uns;
    }

    /**
     * Starts a message pre-bound to this instance — equivalent to
     * {@code MessageBuilder.create(name, version).withConfig(config).withInstance(id())}, so
     * {@code build()} stamps the component identity with this handle's instance token.
     *
     * @param name    the message name (header {@code name})
     * @param version the message version (header {@code version})
     * @return a builder whose built messages carry this instance's identity
     */
    public MessageBuilder newMessage(String name, String version) {
        return MessageBuilder.create(name, version).withConfig(configManager).withInstance(id);
    }

    /**
     * The {@code data()} publish facade bound to this instance (DESIGN-class-facades §2.1): builds +
     * validates the {@code SouthboundSignalUpdate} body (quality → {@code GOOD}, {@code serverTs} →
     * now, samples wrapper), sanitizes the signal path into the {@code data} channel, and routes on
     * the resolved channel (per-call ▸ config {@code publish.channel} ▸ LOCAL).
     *
     * @return the instance-bound {@link DataFacade}
     */
    public DataFacade data() {
        DataFacade bound = data;
        if (bound == null) {
            bound = new DataFacade(configManager, id, uns, messagingClient, streamSink, clock);
            data = bound;
        }
        return bound;
    }

    /**
     * The {@code events()} publish facade bound to this instance (DESIGN-class-facades §2.2):
     * operator events &amp; alarms on the {@code evt} class, deriving the
     * {@code evt/{severity}/{type}} channel from the body.
     *
     * @return the instance-bound {@link EventsFacade}
     */
    public EventsFacade events() {
        EventsFacade bound = events;
        if (bound == null) {
            bound = new EventsFacade(configManager, id, uns, messagingClient, clock);
            events = bound;
        }
        return bound;
    }

    /**
     * The {@code app()} publish facade bound to this instance (DESIGN-class-facades §2.3): free-form
     * inter-component pub/sub on the {@code app} class (named header + verbatim body).
     *
     * @return the instance-bound {@link AppFacade}
     */
    public AppFacade app() {
        AppFacade bound = app;
        if (bound == null) {
            bound = new AppFacade(configManager, id, uns, messagingClient);
            app = bound;
        }
        return bound;
    }
}
