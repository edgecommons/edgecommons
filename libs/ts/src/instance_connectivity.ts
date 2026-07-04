/**
 * Per-instance connectivity reporting for the `state` keepalive (the #1c model).
 *
 * A multi-connection component (an OPC UA adapter with several servers, a Modbus adapter with
 * several slaves, a file-replicator with several source directories) keeps its identity, data and
 * lifecycle under its `main` instance token — it does NOT mint a separate UNS instance per
 * connection. Instead it reports each connection's health here, and the console renders it
 * per-instance under the one component (no phantom "instance" component that never heartbeats).
 *
 * Register a provider with `gg.setInstanceConnectivityProvider(...)`; the `main` `state` keepalive
 * then carries an `instances` array of {@link InstanceConnectivity}.
 */

/** One component instance's southbound/source connectivity. */
export class InstanceConnectivity {
  /**
   * @param instance  the component instance / connection id (OPC UA server id, Modbus slave id,
   *                  replication instance id); must be non-empty.
   * @param connected whether that instance's southbound/source is currently reachable.
   * @param detail    an optional human detail (endpoint, or the down reason).
   */
  constructor(
    public readonly instance: string,
    public readonly connected: boolean,
    public readonly detail?: string,
  ) {
    if (!instance || instance.trim() === "") {
      throw new Error("instance id must be non-empty");
    }
  }

  /** Convenience factory. */
  static of(instance: string, connected: boolean, detail?: string): InstanceConnectivity {
    return new InstanceConnectivity(instance, connected, detail);
  }

  /** The state-body element: `{ instance, connected[, detail] }`. */
  toJson(): Record<string, unknown> {
    const o: Record<string, unknown> = { instance: this.instance, connected: this.connected };
    if (this.detail !== undefined && this.detail.trim() !== "") {
      o.detail = this.detail;
    }
    return o;
  }
}

/**
 * A component-supplied source of per-instance connectivity, sampled each keepalive tick into the
 * state body's `instances` array. Keep it cheap and non-blocking (sample a cached status); an
 * empty/undefined result omits the section. It should not throw — the heartbeat treats a throwing
 * provider as "no data this tick" (best-effort) so a provider bug can never suppress the keepalive.
 */
export type InstanceConnectivityProvider = () => InstanceConnectivity[] | undefined | null;
