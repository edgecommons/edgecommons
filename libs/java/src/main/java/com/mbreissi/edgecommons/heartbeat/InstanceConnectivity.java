/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.heartbeat;

import com.google.gson.JsonElement;
import com.google.gson.JsonObject;

import java.util.LinkedHashMap;
import java.util.Map;

/**
 * One component instance's southbound/source connectivity, reported at the <b>instance level</b>
 * through the component's {@code main} {@code state} keepalive (see
 * {@link InstanceConnectivityProvider}). The UNS model keeps a component's identity, data and
 * lifecycle under its {@code main} instance token — a multi-connection component (e.g. an OPC UA
 * adapter with several servers, a Modbus adapter with several slaves, a file-replicator with several
 * source directories) therefore does NOT mint a separate UNS instance per connection. Instead it
 * reports each connection's health here, and the console renders it per-instance under the one
 * component — no phantom "instance" component that never heartbeats.
 *
 * <p>The same sample answers both surfaces: it is pushed on every {@code state} keepalive tick, and
 * it is what the built-in {@code status} command verb returns when pulled
 * ({@link com.mbreissi.edgecommons.commands.CommandInbox#STATUS}). A component supplies the data
 * once; the library serves it both ways.
 *
 * <h2>The shape</h2>
 * <ul>
 *   <li>{@code instance} — the connection id. Required.</li>
 *   <li>{@code connected} — the <b>normalized</b> reachability flag. Always present, so a console can
 *       render a health dot for any component without knowing that component's vocabulary.</li>
 *   <li>{@code state} — optional, the component's <b>own</b> vocabulary for the richer condition
 *       ({@code ONLINE} / {@code CONNECTING} / {@code BACKOFF} / {@code DISABLED} …). A boolean cannot
 *       distinguish "reconnecting" from "administratively disabled"; this can.</li>
 *   <li>{@code detail} — optional human text (the endpoint, or why it is down).</li>
 *   <li>{@code attributes} — optional open bag of domain data (a camera's capabilities and last error,
 *       an OPC UA server's session id …). Deliberately unconstrained: it is where a component puts
 *       what only it understands, <b>without</b> destabilizing the fields above that everyone relies on.</li>
 * </ul>
 *
 * <p>Serialized into the state body's {@code instances[]} array, and into the {@code status} reply.
 */
public final class InstanceConnectivity
{
    private final String instance;
    private final boolean connected;
    private final String state;
    private final String detail;
    private final Map<String, JsonElement> attributes;

    /**
     * Full constructor.
     *
     * @param instance   the component instance / connection id (e.g. an OPC UA server id, a Modbus
     *                   slave id, a camera id); must be non-null/non-blank
     * @param connected  whether that instance's southbound/source is currently reachable — the
     *                   normalized flag every consumer can read
     * @param detail     an optional human detail (endpoint, or the down reason), or {@code null}
     * @param state      the component's own richer condition token, or {@code null}
     * @param attributes optional domain-specific data, or {@code null}; copied defensively
     *
     * <p>The argument order keeps the pre-existing 3-arg form a strict prefix, which is also the order
     * Python and TypeScript arrived at independently (neither can overload a constructor). One order,
     * four languages.
     */
    public InstanceConnectivity(String instance, boolean connected, String detail, String state,
                                Map<String, JsonElement> attributes)
    {
        if (instance == null || instance.isBlank())
        {
            throw new IllegalArgumentException("instance id must be non-null and non-blank");
        }
        this.instance = instance;
        this.connected = connected;
        this.state = state;
        this.detail = detail;
        this.attributes = attributes == null || attributes.isEmpty()
                ? Map.of()
                : Map.copyOf(new LinkedHashMap<>(attributes));
    }

    /** Retained: the pre-{@code state}/{@code attributes} surface. */
    public InstanceConnectivity(String instance, boolean connected, String detail)
    {
        this(instance, connected, detail, null, null);
    }

    /** Convenience factory without a detail. */
    public static InstanceConnectivity of(String instance, boolean connected)
    {
        return new InstanceConnectivity(instance, connected, null, null, null);
    }

    /** Convenience factory with a detail. */
    public static InstanceConnectivity of(String instance, boolean connected, String detail)
    {
        return new InstanceConnectivity(instance, connected, detail, null, null);
    }

    /** Returns a copy carrying the component's own condition token. */
    public InstanceConnectivity withState(String newState)
    {
        return new InstanceConnectivity(instance, connected, detail, newState, attributes);
    }

    /** Returns a copy carrying domain-specific attributes. */
    public InstanceConnectivity withAttributes(Map<String, JsonElement> newAttributes)
    {
        return new InstanceConnectivity(instance, connected, detail, state, newAttributes);
    }

    public String getInstance()
    {
        return instance;
    }

    public boolean isConnected()
    {
        return connected;
    }

    /** The component's own richer condition token, or {@code null}. */
    public String getState()
    {
        return state;
    }

    /** The optional human detail (endpoint / down reason), or {@code null}. */
    public String getDetail()
    {
        return detail;
    }

    /** The domain-specific attributes; never {@code null}, possibly empty. */
    public Map<String, JsonElement> getAttributes()
    {
        return attributes;
    }

    /**
     * The wire element:
     * {@code {"instance":…,"connected":…[,"state":…][,"detail":…][,"attributes":{…}]}}.
     * Optional members are omitted rather than emitted null, so the common two-field case stays small
     * on a keepalive that ships every 5 seconds per component.
     */
    public JsonObject toJson()
    {
        JsonObject o = new JsonObject();
        o.addProperty("instance", instance);
        o.addProperty("connected", connected);
        if (state != null && !state.isBlank())
        {
            o.addProperty("state", state);
        }
        if (detail != null && !detail.isBlank())
        {
            o.addProperty("detail", detail);
        }
        if (!attributes.isEmpty())
        {
            JsonObject bag = new JsonObject();
            attributes.forEach(bag::add);
            o.add("attributes", bag);
        }
        return o;
    }
}
