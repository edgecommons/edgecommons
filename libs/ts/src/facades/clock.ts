/**
 * The injected-clock seam shared by {@link DataFacade}/{@link EventsFacade} for their `serverTs`/
 * `timestamp` "now" defaults (DESIGN-class-facades §2.1/§2.2) — no inline `Date.now()` calls in
 * the defaulting logic, so the facades unit-test deterministically (mirrors the Java facades'
 * injected `java.time.Clock` and the existing `RepublishListener` `ClockMillis` seam).
 */

/** The injected clock seam (epoch millis). Production default: `() => Date.now()`. */
export type ClockMillis = () => number;

/**
 * Formats epoch millis as an ISO-8601 UTC timestamp (`…Z`), matching the Java facades'
 * `Instant.now(clock).toString()` exactly: JavaScript's `Date.prototype.toISOString()` always
 * emits a `.SSS` millisecond fraction, but `java.time.Instant` omits it when the fraction is
 * exactly zero (a whole second) — so a fixed-clock `2026-07-01T12:00:00Z` must serialize
 * WITHOUT `.000`, or a `uns-test-vectors/{data,evt}.json` conformance case would mismatch on the
 * `serverTs`/`timestamp` string. Any non-zero fraction passes through unchanged.
 */
export function toIso(ms: number): string {
  const iso = new Date(ms).toISOString();
  return iso.endsWith(".000Z") ? iso.slice(0, -5) + "Z" : iso;
}
