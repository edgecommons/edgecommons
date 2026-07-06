/**
 * Error handling — mirrors the Rust `EdgeCommonsError` / Java+Python exception taxonomy.
 *
 * A single {@link EdgeCommonsError} class tagged with a {@link EdgeCommonsErrorKind} stands in for the
 * Rust `thiserror` enum. As in the other libraries, the library itself never exits
 * the process — it returns/throws `EdgeCommonsError` and lets the application decide.
 */

/** The category of a {@link EdgeCommonsError}, mirroring the Rust `EdgeCommonsError` variants. */
export type EdgeCommonsErrorKind =
  | "Cli"
  | "Config"
  | "Validation"
  | "Messaging"
  | "Metrics"
  | "Ipc"
  | "Io"
  | "Json";

/** A edgecommons error, tagged with its {@link EdgeCommonsErrorKind}. */
export class EdgeCommonsError extends Error {
  readonly kind: EdgeCommonsErrorKind;

  constructor(kind: EdgeCommonsErrorKind, message: string) {
    super(message);
    this.name = `EdgeCommonsError(${kind})`;
    this.kind = kind;
  }

  static cli(msg: string): EdgeCommonsError {
    return new EdgeCommonsError("Cli", msg);
  }
  static config(msg: string): EdgeCommonsError {
    return new EdgeCommonsError("Config", msg);
  }
  static validation(msg: string): EdgeCommonsError {
    return new EdgeCommonsError("Validation", msg);
  }
  static messaging(msg: string): EdgeCommonsError {
    return new EdgeCommonsError("Messaging", msg);
  }
  static metrics(msg: string): EdgeCommonsError {
    return new EdgeCommonsError("Metrics", msg);
  }
  static ipc(msg: string): EdgeCommonsError {
    return new EdgeCommonsError("Ipc", msg);
  }
  static io(msg: string): EdgeCommonsError {
    return new EdgeCommonsError("Io", msg);
  }
  static json(msg: string): EdgeCommonsError {
    return new EdgeCommonsError("Json", msg);
  }
}
