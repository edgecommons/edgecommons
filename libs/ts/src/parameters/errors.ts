/** Any parameter-subsystem failure (source/read error, unknown source type, parse error). */
export class ParameterError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "ParameterError";
  }
}
