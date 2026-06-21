"""Credential subsystem error type."""


class CredentialError(Exception):
    """Any vault/credential failure (bad key, tamper, I/O, unimplemented provider).

    Messages never include secret or key material.
    """
