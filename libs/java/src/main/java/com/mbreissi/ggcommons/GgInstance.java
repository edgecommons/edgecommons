/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons;

import com.mbreissi.ggcommons.config.ConfigManager;
import com.mbreissi.ggcommons.messaging.MessageBuilder;
import com.mbreissi.ggcommons.messaging.MessageIdentity;
import com.mbreissi.ggcommons.uns.Uns;

/**
 * The per-instance seam (UNS-CANONICAL-DESIGN §3, D-U3): an instance-scoped handle whose only
 * job is to pre-bind the instance token into (a) the {@link Uns} topic builder and (b) the
 * {@link MessageBuilder}. The messaging client stays instance-agnostic — {@code publish(topic,
 * msg)} already receives both the topic (minted by this handle's instance-bound {@code uns()})
 * and the envelope (stamped by its instance-bound builder).
 *
 * <p>Obtain handles from {@link GGCommons#instance(String)} (validated + cached per id).
 * Component-level messages (everything not built through a handle) default to instance
 * {@value MessageIdentity#DEFAULT_INSTANCE}.
 */
public final class GgInstance {

    private final String id;
    private final ConfigManager configManager;
    private final Uns uns;

    /**
     * Package-private: created by {@link GGCommons#instance(String)}, which validates the token
     * (§2.2 token rule) and caches per id.
     */
    GgInstance(String id, ConfigManager configManager, boolean includeRoot) {
        this.id = id;
        this.configManager = configManager;
        this.uns = new Uns(configManager.getComponentIdentity().withInstance(id), includeRoot);
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
}
