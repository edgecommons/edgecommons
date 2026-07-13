/**
 * Per-instance connectivity reporting for the `state` keepalive and the `status` verb (the #1c model).
 *
 * A multi-connection component (an OPC UA adapter with several servers, a Modbus adapter with
 * several slaves, a file-replicator with several source directories) keeps its identity, data and
 * lifecycle under its `main` instance token — it does NOT mint a separate UNS instance per
 * connection. Instead it reports each connection's health here, and the console renders it
 * per-instance under the one component (no phantom "instance" component that never heartbeats).
 *
 * Register a provider with `gg.setInstanceConnectivityProvider(...)`; the `main` `state` keepalive
 * then carries an `instances` array of {@link InstanceConnectivity}, and the built-in `status`
 * command verb (`CommandInbox.STATUS`) returns the same sample when pulled — one
 * component-supplied provider, two surfaces, no second copy of the data to drift out of step.
 */

/**
 * One component instance's southbound/source connectivity.
 *
 * The shape:
 * - `instance` — the connection id. Required.
 * - `connected` — the **normalized** reachability flag. Always present, so a console can render a
 *   health dot for any component without knowing that component's vocabulary.
 * - `state` — optional, the component's **own** vocabulary for the richer condition (`ONLINE` /
 *   `CONNECTING` / `BACKOFF` / `DISABLED` …). A boolean cannot distinguish "reconnecting" from
 *   "administratively disabled"; this can.
 * - `detail` — optional human text (the endpoint, or why it is down).
 * - `attributes` — optional open bag of domain data (a camera's capabilities and last error, an OPC
 *   UA server's session id …). Deliberately unconstrained: it is where a component puts what only
 *   it understands, **without** destabilizing the fields above that everyone relies on.
 */
export class InstanceConnectivity {
  /** The component instance / connection id. */
  readonly instance: string;
  /** Whether that instance's southbound/source is currently reachable (the normalized flag). */
  readonly connected: boolean;
  /** The optional human detail (endpoint / down reason). */
  readonly detail?: string;
  /** The component's own richer condition token, or `undefined`. */
  readonly state?: string;
  /**
   * The domain-specific attributes; never `undefined` — an absent bag is the empty object (and is
   * omitted from the wire element). Frozen: a defensive copy of what the component supplied.
   */
  readonly attributes: Readonly<Record<string, unknown>>;

  /**
   * @param instance   the component instance / connection id (OPC UA server id, Modbus slave id,
   *                   camera id); must be non-empty.
   * @param connected  whether that instance's southbound/source is currently reachable.
   * @param detail     an optional human detail (endpoint, or the down reason).
   * @param state      the component's own richer condition token.
   * @param attributes optional domain-specific data; copied defensively.
   *
   * **TS-idiom divergence from the Java canonical** (shape only, never the wire): Java's full
   * constructor is `(instance, connected, state, detail, attributes)` with the pre-existing 3-arg
   * `(instance, connected, detail)` retained as an overload. TS constructors cannot be overloaded,
   * so the new members are **appended** after `detail` — every existing 3-arg call keeps working,
   * and the serialized element is byte-identical to Java's either way.
   */
  constructor(
    instance: string,
    connected: boolean,
    detail?: string,
    state?: string,
    attributes?: Record<string, unknown>,
  ) {
    if (!instance || instance.trim() === "") {
      throw new Error("instance id must be non-empty");
    }
    this.instance = instance;
    this.connected = connected;
    this.detail = detail;
    this.state = state;
    this.attributes = Object.freeze(attributes ? { ...attributes } : {});
  }

  /** Convenience factory. */
  static of(instance: string, connected: boolean, detail?: string): InstanceConnectivity {
    return new InstanceConnectivity(instance, connected, detail);
  }

  /** Returns a copy carrying the component's own condition token. */
  withState(state: string): InstanceConnectivity {
    return new InstanceConnectivity(this.instance, this.connected, this.detail, state, { ...this.attributes });
  }

  /** Returns a copy carrying domain-specific attributes. */
  withAttributes(attributes: Record<string, unknown> | undefined): InstanceConnectivity {
    return new InstanceConnectivity(this.instance, this.connected, this.detail, this.state, attributes);
  }

  /**
   * The wire element: `{ instance, connected[, state][, detail][, attributes] }` — carried in the
   * state body's `instances` array and in the `status` reply. Optional members are omitted rather
   * than emitted `null`, so the common two-field case stays small on a keepalive that ships every
   * 5 seconds per component.
   */
  toJson(): Record<string, unknown> {
    const o: Record<string, unknown> = { instance: this.instance, connected: this.connected };
    if (this.state !== undefined && this.state.trim() !== "") {
      o.state = this.state;
    }
    if (this.detail !== undefined && this.detail.trim() !== "") {
      o.detail = this.detail;
    }
    if (Object.keys(this.attributes).length > 0) {
      o.attributes = { ...this.attributes };
    }
    return o;
  }
}

/**
 * A component-supplied source of per-instance connectivity, sampled once per surface through
 * `Heartbeat.sampleInstanceConnectivity()`: into each RUNNING `state` keepalive's `instances`
 * array, and into the built-in `status` verb's reply. Keep it cheap and non-blocking (sample a
 * cached status); an empty/undefined result omits the section. It should not throw — the sampling
 * seam treats a throwing provider as "no data this sample" (best-effort) so a provider bug can
 * never suppress the keepalive or fail the command.
 */
export type InstanceConnectivityProvider = () => InstanceConnectivity[] | undefined | null;
