//! PKCS#11 KEK custodian integration test, exercised against SoftHSM2 (or any PKCS#11 token).
//!
//! Gated: no-op unless `EDGECOMMONS_IT_PKCS11=1` is set. Requires a token with an AES-256 key and
//! these env vars:
//!   PKCS11_MODULE  e.g. /usr/lib/softhsm/libsofthsm2.so
//!   PKCS11_TOKEN   token label   (e.g. ggvault)
//!   PKCS11_KEY     AES key label (e.g. ggvault-kek)
//!   PKCS11_PIN     User PIN
//! SoftHSM also needs SOFTHSM2_CONF pointing at its config (inherited from the environment).

#![cfg(feature = "credentials-pkcs11")]

use edgecommons::credentials::{self, CredentialService, CredentialsConfig, PutOptions};

#[test]
fn pkcs11_wrapped_vault_round_trip_and_persists() {
    if std::env::var("EDGECOMMONS_IT_PKCS11").is_err() {
        eprintln!("skipping PKCS#11 test (set EDGECOMMONS_IT_PKCS11=1 + provide a token)");
        return;
    }
    let module = std::env::var("PKCS11_MODULE").expect("PKCS11_MODULE");
    let token = std::env::var("PKCS11_TOKEN").expect("PKCS11_TOKEN");
    let key = std::env::var("PKCS11_KEY").expect("PKCS11_KEY");
    let pin = std::env::var("PKCS11_PIN").expect("PKCS11_PIN");

    let dir = std::env::temp_dir().join(format!("ggcred-p11-{}", uuid::Uuid::new_v4()));
    let vault_path = dir.join("vault");
    let cfg_json = serde_json::json!({
        "vault": {
            "path": vault_path.to_string_lossy(),
            "keyProvider": {
                "type": "pkcs11",
                "modulePath": module,
                "tokenLabel": token,
                "keyLabel": key,
                "pin": pin,
            }
        }
    });

    // Open a fresh vault (DEK wrapped by the HSM key), write a secret, read it back.
    let cfg: CredentialsConfig = serde_json::from_value(cfg_json.clone()).unwrap();
    let svc = credentials::open(&cfg).expect("open pkcs11 vault");
    svc.put("db/password", b"s3cr3t", PutOptions::default())
        .expect("put");
    let got = svc.get("db/password").expect("get").expect("present");
    assert_eq!(got.bytes(), b"s3cr3t");
    drop(svc);

    // Re-open the persisted vault: the DEK must unwrap through the HSM again (fail-closed otherwise).
    let cfg2: CredentialsConfig = serde_json::from_value(cfg_json).unwrap();
    let svc2 = credentials::open(&cfg2).expect("reopen pkcs11 vault");
    let again = svc2
        .get("db/password")
        .expect("get")
        .expect("present after reopen");
    assert_eq!(again.bytes(), b"s3cr3t");

    let _ = std::fs::remove_dir_all(&dir);
    eprintln!("PKCS#11 vault round-trip OK (module={module})");
}
