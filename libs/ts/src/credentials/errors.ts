/** Any vault/credential failure (bad key, tamper, I/O, unimplemented provider). */
export class CredentialError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "CredentialError";
  }
}
