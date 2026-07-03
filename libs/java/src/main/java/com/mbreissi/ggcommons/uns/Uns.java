/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.uns;

import com.mbreissi.ggcommons.messaging.MessageIdentity;

import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;
import java.util.Objects;

/**
 * The unified-namespace (UNS) topic builder + validator (UNS-CANONICAL-DESIGN §2), bound to a
 * {@link MessageIdentity} and the component's {@code topic.includeRoot} setting. Obtain the
 * component-bound instance via {@code GGCommons.getUns()} (instance
 * {@value MessageIdentity#DEFAULT_INSTANCE}) or an instance-bound one via
 * {@code GGCommons.instance(id).uns()}.
 *
 * <p>Grammar (§2.2): {@code ecv1 [/ {site}]? / {device} / {component} / {instance} / {class}
 * [/ {channel…}]} — the optional {@code site} position (the first hierarchy value) is emitted
 * only when {@code topic.includeRoot} is {@code true}.
 *
 * <p>Normative rules enforced here (each violation throws {@link UnsValidationException} with a
 * machine-readable {@link UnsValidationException.Code}):
 * <ol>
 *   <li><b>Token rule</b> — identical to the config template sanitizer's blacklist, so any
 *       sanitized value builds a publishable topic: a token is non-empty, contains no
 *       {@code / + # \}, no control characters (U+0000–U+001F, U+007F), and no {@code ..}
 *       substring. Dots are legal (a literal within a level). The validator deliberately imposes
 *       no stricter whitelist than the sanitizer.</li>
 *   <li><b>Depth guard</b> — at most {@value #MAX_TOPIC_SLASHES} {@code /} separators total (AWS
 *       IoT Core's 8-level limit), so the channel budget is 3 tokens rootless / 2 tokens
 *       rooted; enforced at build time (an over-deep channel throws, it is never silently
 *       dropped at IoT Core).</li>
 *   <li><b>Length</b> — at most {@value #MAX_TOPIC_UTF8_BYTES} UTF-8 bytes total (the IoT Core
 *       publish limit).</li>
 *   <li><b>Class rules</b> — leaf classes ({@code state}, {@code cfg}) forbid a channel; every
 *       other class requires at least one channel token.</li>
 * </ol>
 *
 * <p>Reply topics ({@code ggcommons/reply-…}) are non-UNS and never pass through this builder.
 */
public final class Uns {

    /** The UNS root literal — the first token of every UNS topic. */
    public static final String ROOT = "ecv1";

    /** AWS IoT Core's 8-level topic limit, expressed as the maximum {@code /} separator count. */
    public static final int MAX_TOPIC_SLASHES = 7;

    /** AWS IoT Core's topic publish limit in UTF-8 bytes. */
    public static final int MAX_TOPIC_UTF8_BYTES = 256;

    private final MessageIdentity identity;
    private final boolean includeRoot;

    /**
     * Creates a topic builder bound to an identity and a root mode. Library-internal wiring —
     * components obtain bound instances from the {@code GGCommons} facade.
     *
     * @param identity    the identity whose tokens {@link #topic(UnsClass)} emits (non-null)
     * @param includeRoot whether topics/filters carry the first hierarchy value ({@code site})
     *                    between the {@value #ROOT} root and the device ({@code topic.includeRoot},
     *                    default {@code false})
     */
    public Uns(MessageIdentity identity, boolean includeRoot) {
        this.identity = Objects.requireNonNull(identity, "identity must not be null");
        this.includeRoot = includeRoot;
    }

    /** Returns the bound identity. */
    public MessageIdentity identity() {
        return identity;
    }

    /**
     * Builds the bound identity's concrete topic for a <b>leaf</b> class ({@code state},
     * {@code cfg}) — or, for a channeled class, throws {@code CHANNEL_REQUIRED} (use
     * {@link #topic(UnsClass, String)}).
     *
     * @param cls the UNS class (non-null)
     * @return the concrete topic, e.g. {@code ecv1/gw-01/opcua-adapter/main/state}
     * @throws UnsValidationException on any §2.2 violation
     */
    public String topic(UnsClass cls) {
        return topicFor(identity, cls, null);
    }

    /**
     * Builds the bound identity's concrete topic for a channeled class.
     *
     * @param cls     the UNS class (non-null)
     * @param channel the channel — one or more {@code /}-separated tokens (≤ 3 rootless,
     *                ≤ 2 rooted), e.g. {@code "temp"} or {@code "sb/status"}; {@code null}/empty
     *                means "no channel" (only legal for leaf classes)
     * @return the concrete topic, e.g. {@code ecv1/gw-01/opcua-adapter/main/data/temp}
     * @throws UnsValidationException on any §2.2 violation
     */
    public String topic(UnsClass cls, String channel) {
        return topicFor(identity, cls, channel);
    }

    /**
     * Builds a concrete topic for a <b>peer's</b> identity — typically a received message's
     * {@code getIdentity()} — which is how a component addresses a peer's {@code cmd} inbox
     * without parsing topics. The target's tokens pass the same token rule as the bound
     * identity's (a foreign identity with unsanitized values fails to build, it never produces
     * an unpublishable topic).
     *
     * @param target  the peer identity to mint the topic for (non-null)
     * @param cls     the UNS class (non-null)
     * @param channel the channel tokens ({@code null}/empty for leaf classes)
     * @return the concrete topic for the target identity
     * @throws UnsValidationException on any §2.2 violation
     */
    public String topicFor(MessageIdentity target, UnsClass cls, String channel) {
        Objects.requireNonNull(target, "target identity must not be null");
        Objects.requireNonNull(cls, "class must not be null");
        List<String> segments = new ArrayList<>(MAX_TOPIC_SLASHES + 1);
        segments.add(ROOT);
        if (includeRoot) {
            segments.add(checkedToken(target.getHier().get(0).value(), "site (hier[0]) value"));
        }
        segments.add(checkedToken(target.getDevice(), "device"));
        segments.add(checkedToken(target.getComponent(), "component"));
        segments.add(checkedToken(target.getInstance(), "instance"));
        segments.add(cls.token);

        boolean channelSupplied = channel != null && !channel.isEmpty();
        if (cls.leaf && channelSupplied) {
            throw new UnsValidationException(UnsValidationException.Code.CHANNEL_ON_LEAF,
                    "class '" + cls.token + "' is a leaf class - a channel is forbidden (got '"
                            + channel + "')");
        }
        if (!cls.leaf && !channelSupplied) {
            throw new UnsValidationException(UnsValidationException.Code.CHANNEL_REQUIRED,
                    "class '" + cls.token + "' requires at least one channel token");
        }
        if (channelSupplied) {
            for (String channelToken : channel.split("/", -1)) {
                segments.add(checkedToken(channelToken, "channel token"));
            }
        }

        String topic = String.join("/", segments);
        int slashes = segments.size() - 1;
        if (slashes > MAX_TOPIC_SLASHES) {
            throw new UnsValidationException(UnsValidationException.Code.DEPTH_EXCEEDED,
                    "topic '" + topic + "' has " + slashes + " '/' separators (max "
                            + MAX_TOPIC_SLASHES + "; the channel budget is "
                            + (includeRoot ? 2 : 3) + " token(s) with topic.includeRoot="
                            + includeRoot + ")");
        }
        checkLength(topic);
        return topic;
    }

    /**
     * Builds a subscription filter for a class over a wildcard {@link UnsScope}: {@code null}
     * scope fields render as {@code +}; channeled classes get a trailing {@code /#} (all
     * channels); leaf classes end at the class token. The {@code site} position exists (and
     * {@link UnsScope#site()} is consulted) only when {@code topic.includeRoot} is {@code true}.
     *
     * <p>The output is correct by construction and is NOT passed through {@link #validate}
     * (filters legitimately carry wildcards).
     *
     * @param cls   the UNS class (non-null)
     * @param scope the wildcard scope (non-null; use {@link UnsScope#all()} for everything)
     * @return the subscription filter, e.g. {@code ecv1/+/+/+/data/#}
     * @throws UnsValidationException when a pinned (non-null) scope field violates the token rule
     */
    public String filter(UnsClass cls, UnsScope scope) {
        Objects.requireNonNull(cls, "class must not be null");
        Objects.requireNonNull(scope, "scope must not be null (use UnsScope.all())");
        List<String> segments = new ArrayList<>(MAX_TOPIC_SLASHES + 1);
        segments.add(ROOT);
        if (includeRoot) {
            segments.add(wildcardOr(scope.site(), "site"));
        }
        segments.add(wildcardOr(scope.device(), "device"));
        segments.add(wildcardOr(scope.component(), "component"));
        segments.add(wildcardOr(scope.instance(), "instance"));
        segments.add(cls.token);
        String filter = String.join("/", segments);
        return cls.leaf ? filter : filter + "/#";
    }

    /**
     * Validates a <b>concrete</b> topic against the full §2.2 grammar under this instance's root
     * mode: wildcards are rejected ({@code WILDCARD_IN_TOPIC}); every token passes the token
     * rule; the first token must be the {@value #ROOT} root literal; depth ≤
     * {@value #MAX_TOPIC_SLASHES} separators; length ≤ {@value #MAX_TOPIC_UTF8_BYTES} UTF-8
     * bytes; the class position (5th token rootless, 6th rooted) must hold a {@link UnsClass}
     * token; leaf classes must end at the class token and channeled classes must carry at least
     * one channel token.
     *
     * @param topic the concrete topic to validate
     * @throws UnsValidationException with the precise {@link UnsValidationException.Code} on the
     *                                first violation found
     */
    public void validate(String topic) {
        if (topic == null || topic.isEmpty()) {
            throw new UnsValidationException(UnsValidationException.Code.EMPTY_TOKEN,
                    "topic is null or empty");
        }
        if (topic.indexOf('+') >= 0 || topic.indexOf('#') >= 0) {
            throw new UnsValidationException(UnsValidationException.Code.WILDCARD_IN_TOPIC,
                    "validate() accepts only concrete topics - '" + topic
                            + "' contains an MQTT wildcard ('+'/'#')");
        }
        String[] tokens = topic.split("/", -1);
        for (String token : tokens) {
            checkToken(token, "topic token");
        }
        if (!ROOT.equals(tokens[0])) {
            throw new UnsValidationException(UnsValidationException.Code.BAD_ROOT,
                    "topic '" + topic + "' must start with the UNS root '" + ROOT + "' (got '"
                            + tokens[0] + "')");
        }
        int slashes = tokens.length - 1;
        if (slashes > MAX_TOPIC_SLASHES) {
            throw new UnsValidationException(UnsValidationException.Code.DEPTH_EXCEEDED,
                    "topic '" + topic + "' has " + slashes + " '/' separators (max "
                            + MAX_TOPIC_SLASHES + ")");
        }
        checkLength(topic);
        int classPosition = includeRoot ? 5 : 4;
        if (tokens.length <= classPosition) {
            throw new UnsValidationException(UnsValidationException.Code.BAD_CLASS,
                    "topic '" + topic + "' has too few levels (" + tokens.length + "): the class"
                            + " token is expected at position " + classPosition
                            + " (topic.includeRoot=" + includeRoot + ")");
        }
        UnsClass cls = UnsClass.fromToken(tokens[classPosition]);
        if (cls == null) {
            throw new UnsValidationException(UnsValidationException.Code.BAD_CLASS,
                    "'" + tokens[classPosition] + "' (position " + classPosition + " of '" + topic
                            + "') is not a UNS class token");
        }
        boolean hasChannel = tokens.length > classPosition + 1;
        if (cls.leaf && hasChannel) {
            throw new UnsValidationException(UnsValidationException.Code.CHANNEL_ON_LEAF,
                    "class '" + cls.token + "' is a leaf class - topic '" + topic
                            + "' must end at the class token");
        }
        if (!cls.leaf && !hasChannel) {
            throw new UnsValidationException(UnsValidationException.Code.CHANNEL_REQUIRED,
                    "class '" + cls.token + "' requires at least one channel token - topic '"
                            + topic + "' ends at the class token");
        }
    }

    /**
     * The §2.2 <b>token rule</b> — deliberately the SAME blacklist as the config template
     * sanitizer ({@code ConfigManager.sanitize}), so any sanitized value passes: non-empty, no
     * {@code / + # \}, no control characters (U+0000–U+001F, U+007F), no {@code ..} substring.
     * Also the validation gate for {@code GGCommons.instance(id)} instance tokens.
     *
     * @param token the token to check
     * @param what  what the token is, for the error message (e.g. {@code "instance id"})
     * @throws UnsValidationException {@code EMPTY_TOKEN} / {@code BAD_CHAR} / {@code TRAVERSAL}
     */
    public static void checkToken(String token, String what) {
        if (token == null || token.isEmpty()) {
            throw new UnsValidationException(UnsValidationException.Code.EMPTY_TOKEN,
                    what + " must be a non-empty token");
        }
        for (int i = 0; i < token.length(); i++) {
            char c = token.charAt(i);
            if (c == '/' || c == '+' || c == '#' || c == '\\' || c < 0x20 || c == 0x7F) {
                throw new UnsValidationException(UnsValidationException.Code.BAD_CHAR,
                        what + " '" + token + "' contains a forbidden character at index " + i
                                + " (no '/', '+', '#', '\\' or control characters)");
            }
        }
        if (token.contains("..")) {
            throw new UnsValidationException(UnsValidationException.Code.TRAVERSAL,
                    what + " '" + token + "' contains the traversal sequence '..'");
        }
    }

    /** {@link #checkToken} that returns the (valid) token, for inline segment assembly. */
    private static String checkedToken(String token, String what) {
        checkToken(token, what);
        return token;
    }

    /** Renders a scope field: {@code null} as the {@code +} wildcard, else the checked token. */
    private static String wildcardOr(String value, String what) {
        return value == null ? "+" : checkedToken(value, what);
    }

    /** Enforces the {@value #MAX_TOPIC_UTF8_BYTES}-UTF-8-byte topic length limit. */
    private static void checkLength(String topic) {
        int bytes = topic.getBytes(StandardCharsets.UTF_8).length;
        if (bytes > MAX_TOPIC_UTF8_BYTES) {
            throw new UnsValidationException(UnsValidationException.Code.LENGTH_EXCEEDED,
                    "topic is " + bytes + " UTF-8 bytes (max " + MAX_TOPIC_UTF8_BYTES + ")");
        }
    }
}
