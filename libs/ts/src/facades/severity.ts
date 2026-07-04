/**
 * The operator-event severity taxonomy (DESIGN-class-facades §2.2). The wire token is the enum's
 * **lowercase** value — `critical | warning | info | debug` — and it is **the first channel
 * token** of every `evt` publish: {@link EventsFacade} (`facades/events_facade.ts`) derives the
 * channel `evt/{severity}/{type}` from the body's own severity + type, so the topic and the body
 * can never disagree. A console subscribes `ecv1/+/+/+/evt/critical/#` for just alarms.
 */
export enum Severity {
  /** An alarm-grade condition demanding operator attention (the `raiseAlarm` default). */
  Critical = "critical",
  /** A degraded but non-critical condition. */
  Warning = "warning",
  /** An informational event (the message-only `emit` convenience default). */
  Info = "info",
  /** A diagnostic event. */
  Debug = "debug",
}

/**
 * Resolves a lowercase wire token to its {@link Severity}, or `undefined` when the token is
 * outside the closed set (wire tokens are case-sensitive — `"INFO"` does not match).
 */
export function severityFromWire(token: string): Severity | undefined {
  return (Object.values(Severity) as string[]).includes(token) ? (token as Severity) : undefined;
}
