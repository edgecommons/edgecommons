/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.heartbeat;

import com.google.gson.JsonObject;

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
 * <p>Minimal + extensible: an instance {@code id}, whether it is currently {@code connected}, and an
 * optional human {@code detail} (the endpoint, or the reason it is down). Serialized into the state
 * body's {@code instances[]} array.
 */
public final class InstanceConnectivity
{
    private final String instance;
    private final boolean connected;
    private final String detail;

    /**
     * @param instance  the component instance / connection id (e.g. an OPC UA server id, a Modbus
     *                  slave id, a replication instance id); must be non-null/non-blank
     * @param connected whether that instance's southbound/source is currently reachable
     * @param detail    an optional human detail (endpoint, or the down reason), or {@code null}
     */
    public InstanceConnectivity(String instance, boolean connected, String detail)
    {
        if (instance == null || instance.isBlank())
        {
            throw new IllegalArgumentException("instance id must be non-null and non-blank");
        }
        this.instance = instance;
        this.connected = connected;
        this.detail = detail;
    }

    /** Convenience factory without a detail. */
    public static InstanceConnectivity of(String instance, boolean connected)
    {
        return new InstanceConnectivity(instance, connected, null);
    }

    /** Convenience factory with a detail. */
    public static InstanceConnectivity of(String instance, boolean connected, String detail)
    {
        return new InstanceConnectivity(instance, connected, detail);
    }

    public String getInstance()
    {
        return instance;
    }

    public boolean isConnected()
    {
        return connected;
    }

    /** The optional human detail (endpoint / down reason), or {@code null}. */
    public String getDetail()
    {
        return detail;
    }

    /** The state-body element: {@code {"instance":…,"connected":…[,"detail":…]}}. */
    public JsonObject toJson()
    {
        JsonObject o = new JsonObject();
        o.addProperty("instance", instance);
        o.addProperty("connected", connected);
        if (detail != null && !detail.isBlank())
        {
            o.addProperty("detail", detail);
        }
        return o;
    }
}
