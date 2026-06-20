/**
 * Error handling — mirrors the Rust `GgError` / Java+Python exception taxonomy.
 *
 * A single {@link GgError} class tagged with a {@link GgErrorKind} stands in for the
 * Rust `thiserror` enum. As in the other libraries, the library itself never exits
 * the process — it returns/throws `GgError` and lets the application decide.
 */

/** The category of a {@link GgError}, mirroring the Rust `GgError` variants. */
export type GgErrorKind =
  | "Cli"
  | "Config"
  | "Validation"
  | "Messaging"
  | "Metrics"
  | "Ipc"
  | "Io"
  | "Json";

/** A ggcommons error, tagged with its {@link GgErrorKind}. */
export class GgError extends Error {
  readonly kind: GgErrorKind;

  constructor(kind: GgErrorKind, message: string) {
    super(message);
    this.name = `GgError(${kind})`;
    this.kind = kind;
  }

  static cli(msg: string): GgError {
    return new GgError("Cli", msg);
  }
  static config(msg: string): GgError {
    return new GgError("Config", msg);
  }
  static validation(msg: string): GgError {
    return new GgError("Validation", msg);
  }
  static messaging(msg: string): GgError {
    return new GgError("Messaging", msg);
  }
  static metrics(msg: string): GgError {
    return new GgError("Metrics", msg);
  }
  static ipc(msg: string): GgError {
    return new GgError("Ipc", msg);
  }
  static io(msg: string): GgError {
    return new GgError("Io", msg);
  }
  static json(msg: string): GgError {
    return new GgError("Json", msg);
  }
}
