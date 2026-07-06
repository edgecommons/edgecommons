/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.heartbeat;

import java.util.List;

/**
 * A component-supplied source of per-instance connectivity, sampled on every {@code state} keepalive
 * tick and emitted in the state body's {@code instances[]} array. Register one with
 * {@code gg.setInstanceConnectivityProvider(...)} (which forwards to
 * {@link Heartbeat#setInstanceConnectivityProvider}).
 *
 * <p>This is the overridable, per-language surface for reporting connectivity <b>at the instance
 * level</b> without giving each connection its own UNS instance identity (data + lifecycle stay
 * under {@code main} — see {@link InstanceConnectivity}). A reference adapter implements it by
 * mapping each configured connection to its live reachability:
 * <ul>
 *   <li>OPC UA → per-server session connectivity;</li>
 *   <li>Modbus → per-slave reachability;</li>
 *   <li>file-replicator → whether each instance's source directory is available.</li>
 * </ul>
 *
 * <p>Called on the heartbeat thread each tick; keep it cheap and non-blocking (sample a cached
 * status, don't do IO). It must not throw — the heartbeat treats a thrown provider as "no instance
 * data this tick" (best-effort) so a provider bug can never suppress the state keepalive.
 */
@FunctionalInterface
public interface InstanceConnectivityProvider
{
    /**
     * @return the current per-instance connectivity; an empty or {@code null} list omits the
     *         {@code instances[]} section for this tick
     */
    List<InstanceConnectivity> instanceConnectivity();
}
