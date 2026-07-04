/**
 * The normalized, protocol-independent sample-quality verdict of the southbound contract
 * (DESIGN-class-facades §2.1, `docs/SOUTHBOUND.md` §3). The wire token is the enum's
 * **UPPERCASE** value — `GOOD | BAD | UNCERTAIN` — carried verbatim on every `data` sample.
 * Mirrors the `UnsClass` idiom already used in this library: the enum's own string value IS the
 * wire token, so no separate `wire()` accessor is needed.
 *
 * {@link DataFacade} (`facades/data_facade.ts`) defaults an omitted sample quality to
 * {@link Quality.Good} (marking the synthesis with `qualityRaw:"unspecified"`), so a sample can
 * never reach the bus without a quality — the structural guarantee the facade exists to make.
 */
export enum Quality {
  /** The value is trustworthy (the default for a sample carrying a value with no verdict). */
  Good = "GOOD",
  /** The value is not trustworthy (exception/timeout/failed read). */
  Bad = "BAD",
  /** The value is present but suspect (stale/partial). */
  Uncertain = "UNCERTAIN",
}

/**
 * Resolves an UPPERCASE wire token to its {@link Quality}, or `undefined` when the token is
 * outside the closed set (wire tokens are case-sensitive — `"good"` does not match).
 */
export function qualityFromWire(token: string): Quality | undefined {
  return (Object.values(Quality) as string[]).includes(token) ? (token as Quality) : undefined;
}
