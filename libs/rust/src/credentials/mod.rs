//! # Credentials & local vault (`credentials` feature)
//!
//! **One-liner purpose**: A generic, secure secret store for components — an encrypted-at-rest
//! local vault that runs standalone or (later phases) is seeded and refreshed from a central
//! cloud vault. Peer subsystem to `config`/`messaging`/`metrics`; obtained via `gg.credentials()`.
//!
//! ## Overview
//! Secrets are **named, versioned, opaque byte blobs**. The vault is one encrypted JSON file with a
//! normative, cross-language byte format (see [`format`]) so the Java/Python/TS ports interoperate
//! with the same on-disk vault. Encryption is envelope-based: a per-vault Data Encryption Key
//! (DEK) seals records (AES-256-GCM); the DEK is wrapped by a Key Encryption Key from a pluggable
//! [`keyprovider::KeyProvider`] (phase 1: `file`).
//!
//! ## Semantics & Architecture
//! - **Shared device vault** model: a single file written by one owner under an advisory lock,
//!   read by many components (lock-free reads + reload-on-change).
//! - **Fail-closed**: a wrong KEK, tampered file, or AAD mismatch returns an error, never
//!   plaintext. Key material and decrypted values live in [`zeroize`]-ing buffers and are never
//!   logged ([`Secret`]'s `Debug` redacts).
//!
//! ## Usage
//! ```no_run
//! # #[cfg(feature = "credentials")] {
//! use edgecommons::credentials::{self, CredentialsConfig};
//! let cfg = CredentialsConfig::default();              // file provider, local-only vault
//! let creds = credentials::open(&cfg).unwrap();
//! use edgecommons::credentials::CredentialService;
//! creds.put("db/password", b"s3cr3t", Default::default()).unwrap();
//! let pw = creds.get_string("db/password").unwrap().unwrap();
//! # }
//! ```
//!
//! ## Related
//! Design: `docs/CREDENTIALS.md`. Streaming sinks will consume this in phase 3 (closes that
//! design's §7).

pub mod bridge;
pub mod central;
pub mod config;
pub mod audit;
mod crypto;
pub mod format;
pub mod keyprovider;
pub mod secretref;
pub mod service;
pub mod sync;
pub mod vault;
pub mod views;

pub use audit::{AuditEvent, AuditSink, LogAuditSink};
pub use central::{CentralSecret, CentralVaultSource};
pub use config::{open, open_namespaced, open_namespaced_with_default, AuditConfig, CentralConfig, CredentialsConfig, KeyProviderConfig, SyncEntry, SyncSelect, VaultConfig};
// Convenience re-export consumed only by the parameter cache (`parameters` feature). The
// credentials module itself calls the function via its `config::` path, so gate the re-export to
// avoid an unused-import warning in credentials-only builds.
#[cfg(feature = "parameters")]
pub(crate) use config::build_key_provider;
pub use keyprovider::{EnvKeyProvider, FileKeyProvider, KeyProvider, DEFAULT_KEK_ENV_VAR};
pub use secretref::resolve_secret_refs;
pub use bridge::CredentialMetricsBridge;
pub use service::{CredentialService, CredentialStats, DefaultCredentialService, Secret, SecretMeta};
pub use sync::SyncEngine;
pub use vault::{LocalVault, PutOptions};
pub use views::{AwsCredentials, BasicAuth, KafkaSasl, TlsBundle};

#[cfg(feature = "credentials-aws")]
pub use central::AwsSecretsManagerSource;
#[cfg(feature = "credentials-aws")]
pub use keyprovider::KmsKeyProvider;
#[cfg(feature = "credentials-pkcs11")]
pub use keyprovider::Pkcs11KeyProvider;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn file_vault(dir: &std::path::Path) -> DefaultCredentialService {
        let provider = Arc::new(FileKeyProvider::from_bytes([7u8; 32])) as Arc<dyn KeyProvider>;
        let vault = LocalVault::open(dir.join("vault"), provider, 2).unwrap();
        DefaultCredentialService::new(vault)
    }

    #[test]
    fn audit_emits_access_events_without_values() {
        use std::sync::Mutex;

        struct VecSink(Mutex<Vec<String>>);
        impl AuditSink for VecSink {
            fn record(&self, e: &AuditEvent) {
                self.0.lock().unwrap().push(format!("{}:{}:{}", e.op, e.name, e.outcome));
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let sink = Arc::new(VecSink(Mutex::new(Vec::new())));
        let c = file_vault(dir.path()).with_audit(Some(sink.clone() as Arc<dyn AuditSink>));

        c.put("db/password", b"s3cr3t", PutOptions::default()).unwrap();
        c.get("db/password").unwrap();
        c.get("missing").unwrap();
        c.delete("db/password").unwrap();

        let events = sink.0.lock().unwrap().clone();
        assert!(events.contains(&"put:db/password:ok".to_string()), "{events:?}");
        assert!(events.contains(&"get:db/password:hit".to_string()), "{events:?}");
        assert!(events.contains(&"get:missing:miss".to_string()), "{events:?}");
        assert!(events.contains(&"delete:db/password:ok".to_string()), "{events:?}");
        // The value must never appear in any audit record.
        assert!(events.iter().all(|e| !e.contains("s3cr3t")), "{events:?}");
    }

    #[test]
    fn audit_disabled_by_default_emits_nothing() {
        // A service with no audit sink (default) must not panic and must record nothing.
        let dir = tempfile::tempdir().unwrap();
        let c = file_vault(dir.path());
        c.put("k", b"v", PutOptions::default()).unwrap();
        let _ = c.get("k").unwrap();
        // No sink wired -> no observable audit; the ops just succeed.
    }

    #[test]
    fn put_get_roundtrip_and_typed_views() {
        let dir = tempfile::tempdir().unwrap();
        let c = file_vault(dir.path());
        c.put("db/password", b"s3cr3t", PutOptions::default()).unwrap();
        c.put("svc/config", br#"{"k":1}"#, PutOptions::default()).unwrap();

        assert_eq!(c.get_string("db/password").unwrap().unwrap(), "s3cr3t");
        assert_eq!(c.get_json("svc/config").unwrap().unwrap()["k"], 1);
        assert!(c.exists("db/password").unwrap());
        assert!(c.get("missing").unwrap().is_none());

        let names: Vec<_> = c.list("").unwrap().into_iter().map(|m| m.name).collect();
        assert_eq!(names, vec!["db/password", "svc/config"]);
    }

    #[test]
    fn typed_views_parse() {
        let dir = tempfile::tempdir().unwrap();
        let c = file_vault(dir.path());
        c.put("aws", br#"{"accessKeyId":"AKIA","secretAccessKey":"sk","sessionToken":"tok"}"#, PutOptions::default()).unwrap();
        c.put("basic", br#"{"username":"u","password":"p"}"#, PutOptions::default()).unwrap();
        c.put("tls", br#"{"certPem":"C","keyPem":"K"}"#, PutOptions::default()).unwrap();
        c.put("kafka", br#"{"username":"ku","password":"kp"}"#, PutOptions::default()).unwrap();

        let aws = c.get_aws_credentials("aws").unwrap().unwrap();
        assert_eq!(aws.access_key_id, "AKIA");
        assert_eq!(aws.session_token.as_deref(), Some("tok"));
        assert_eq!(c.get_basic_auth("basic").unwrap().unwrap().username, "u");
        assert_eq!(c.get_tls_bundle("tls").unwrap().unwrap().cert_pem, "C");
        let k = c.get_kafka_sasl("kafka").unwrap().unwrap();
        assert_eq!(k.username, "ku");
        assert_eq!(k.mechanism, "PLAIN"); // default
        // Wrong shape is a typed error, not a silent None.
        assert!(c.get_aws_credentials("basic").is_err());
        assert!(c.get_basic_auth("missing").unwrap().is_none());
    }

    #[test]
    fn versions_are_monotonic_and_pruned() {
        let dir = tempfile::tempdir().unwrap();
        let c = file_vault(dir.path()); // keep_versions = 2
        c.put("k", b"v1", PutOptions::default()).unwrap();
        c.put("k", b"v2", PutOptions::default()).unwrap();
        c.put("k", b"v3", PutOptions::default()).unwrap();
        // Only the newest 2 retained.
        assert_eq!(c.versions("k").unwrap(), vec!["00000002", "00000003"]);
        assert_eq!(c.get("k").unwrap().unwrap().as_str().unwrap(), "v3");
        assert_eq!(c.get_version("k", "00000002").unwrap().unwrap().as_str().unwrap(), "v2");
        assert!(c.get_version("k", "00000001").unwrap().is_none());
    }

    #[test]
    fn stats_reports_secret_count() {
        let dir = tempfile::tempdir().unwrap();
        let c = file_vault(dir.path());
        c.put("a", b"1", PutOptions::default()).unwrap();
        c.put("b", b"2", PutOptions::default()).unwrap();
        let s = c.stats();
        assert_eq!(s.secret_count, 2);
        assert_eq!(s.sync_failures, 0);
        assert_eq!(s.rotations, 0);
        assert!(s.last_sync_age_ms.is_none()); // no central sync configured
    }

    #[test]
    fn secret_refs_resolve_from_vault() {
        let dir = tempfile::tempdir().unwrap();
        let c = file_vault(dir.path());
        c.put("kafka/pw", b"s3cr3t", PutOptions::default()).unwrap();
        c.put("kafka/sasl", br#"{"username":"u","password":"p"}"#, PutOptions::default()).unwrap();

        let mut cfg = serde_json::json!({
            "sink": { "type": "kafka", "properties": {
                "sasl.password": { "$secret": "kafka/pw" },
                "sasl.username": { "$secret": "kafka/sasl", "field": "username" }
            }},
            "plain": "untouched"
        });
        super::resolve_secret_refs(&mut cfg, &c).unwrap();
        assert_eq!(cfg["sink"]["properties"]["sasl.password"], "s3cr3t");
        assert_eq!(cfg["sink"]["properties"]["sasl.username"], "u");
        assert_eq!(cfg["plain"], "untouched");

        let mut missing = serde_json::json!({ "x": { "$secret": "nope" } });
        assert!(super::resolve_secret_refs(&mut missing, &c).is_err());
    }

    #[test]
    fn namespacing_isolates_components_in_a_shared_vault() {
        use std::sync::Mutex;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vault");
        let kek = [5u8; 32];
        let open = || {
            let p = Arc::new(FileKeyProvider::from_bytes(kek)) as Arc<dyn KeyProvider>;
            Arc::new(Mutex::new(LocalVault::open(&path, p, 2).unwrap()))
        };
        // Two components, one shared device vault, transparent namespaces.
        let comp1 = DefaultCredentialService::with_sync(open(), None, "thing-1/CompA".to_string());
        let comp2 = DefaultCredentialService::with_sync(open(), None, "thing-1/CompB".to_string());

        comp1.put("db/password", b"a-secret", PutOptions::default()).unwrap();
        comp2.put("db/password", b"b-secret", PutOptions::default()).unwrap();

        // Same caller-facing key, no collision: each sees only its own value.
        assert_eq!(comp1.get_string("db/password").unwrap().unwrap(), "a-secret");
        assert_eq!(comp2.get_string("db/password").unwrap().unwrap(), "b-secret");
        // list() is scoped to the component's namespace and returns the relative name.
        assert_eq!(comp1.list("").unwrap().iter().map(|m| m.name.clone()).collect::<Vec<_>>(), vec!["db/password"]);

        // On disk both are present under distinct namespaced keys.
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("thing-1/CompA/db/password"));
        assert!(raw.contains("thing-1/CompB/db/password"));
    }

    #[test]
    fn persists_and_reopens_with_same_key() {
        let dir = tempfile::tempdir().unwrap();
        {
            let c = file_vault(dir.path());
            c.put("token", b"abc", PutOptions::default()).unwrap();
        }
        // Reopen with the same KEK — must decrypt.
        let c2 = file_vault(dir.path());
        assert_eq!(c2.get_string("token").unwrap().unwrap(), "abc");
    }

    #[test]
    fn wrong_kek_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        {
            let c = file_vault(dir.path());
            c.put("token", b"abc", PutOptions::default()).unwrap();
        }
        // Different KEK → DEK unwrap / integrity fails; never returns plaintext.
        let provider = Arc::new(FileKeyProvider::from_bytes([9u8; 32])) as Arc<dyn KeyProvider>;
        let err = LocalVault::open(dir.path().join("vault"), provider, 2);
        assert!(err.is_err(), "opening with the wrong KEK must fail");
    }

    #[test]
    fn tamper_is_detected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vault");
        {
            let c = file_vault(dir.path());
            c.put("k", b"v1", PutOptions::default()).unwrap();
        }
        // Flip a byte inside a ciphertext field; reopening must fail the integrity check.
        let mut text = std::fs::read_to_string(&path).unwrap();
        let mut vf: serde_json::Value = serde_json::from_str(&text).unwrap();
        let ct = vf["secrets"]["k"]["versions"][0]["ciphertext"].as_str().unwrap().to_string();
        let mut bytes = base64::engine::general_purpose::STANDARD.decode(&ct).unwrap();
        bytes[0] ^= 0x01;
        vf["secrets"]["k"]["versions"][0]["ciphertext"] =
            serde_json::Value::String(base64::engine::general_purpose::STANDARD.encode(&bytes));
        text = serde_json::to_string(&vf).unwrap();
        std::fs::write(&path, text).unwrap();

        let provider = Arc::new(FileKeyProvider::from_bytes([7u8; 32])) as Arc<dyn KeyProvider>;
        assert!(LocalVault::open(&path, provider, 2).is_err(), "tampered vault must fail to open");
    }

    use base64::Engine as _;

    // Generates (first run) and then verifies the cross-language conformance vectors under
    // `vault-test-vectors/` at the repo root. The Java/Python/TS ports load these same files and
    // must (a) decrypt the canonical vault and (b) reproduce the ciphertext/MAC bytes exactly.
    // Re-running asserts the reference is deterministic (disk == freshly recomputed).
    #[test]
    fn cross_language_test_vectors() {
        use super::crypto;
        use super::format::{self, KekInfo, SecretEntry, VaultFile, VersionEntry, FORMAT_VERSION};
        use std::collections::BTreeMap;
        let b64 = base64::engine::general_purpose::STANDARD;

        // Fixed inputs (no randomness) so every language computes identical bytes.
        let kek: [u8; 32] = std::array::from_fn(|i| i as u8); // 00..1f
        let dek: [u8; 32] = std::array::from_fn(|i| (0x40 + i) as u8); // 40..5f
        let vault_id = "00000000-0000-4000-8000-000000000001";
        let wrap_nonce: [u8; 12] = std::array::from_fn(|i| (0xA0 + i) as u8);

        let records: [(&str, &str, [u8; 12], &[u8]); 2] = [
            ("alpha", "00000001", std::array::from_fn(|i| (0xB0 + i) as u8), b"hello"),
            ("beta", "00000001", std::array::from_fn(|i| (0xC0 + i) as u8), br#"{"x":1}"#),
        ];

        let wrapped_dek = crypto::seal(&kek, &wrap_nonce, &format::dek_wrap_aad(vault_id), &dek).unwrap();
        let mut secrets: BTreeMap<String, SecretEntry> = BTreeMap::new();
        let mut record_vectors = Vec::new();
        for (name, version, nonce, plaintext) in records {
            let ct = crypto::seal(&dek, &nonce, &format::record_aad(vault_id, name, version), plaintext).unwrap();
            secrets.insert(
                name.to_string(),
                SecretEntry {
                    versions: vec![VersionEntry {
                        version: version.to_string(),
                        created_ms: 1_700_000_000_000,
                        ttl_secs: None,
                        source: "local".to_string(),
                        central_version_id: None,
                        labels: BTreeMap::new(),
                        content_type: "application/octet-stream".to_string(),
                        nonce: b64.encode(nonce),
                        ciphertext: b64.encode(&ct),
                    }],
                },
            );
            record_vectors.push(serde_json::json!({
                "name": name, "version": version,
                "nonceB64": b64.encode(nonce),
                "plaintextB64": b64.encode(plaintext),
                "ciphertextB64": b64.encode(&ct),
            }));
        }

        let mac_key = crypto::derive_mac_key(&dek, vault_id);
        let mac_input = format::mac_input(vault_id, &secrets, |s| b64.decode(s).unwrap());
        let mac = b64.encode(crypto::hmac(&mac_key, &mac_input));

        let vault = VaultFile {
            format: FORMAT_VERSION,
            vault_id: vault_id.to_string(),
            kek: KekInfo {
                provider: "file".to_string(),
                alg: "AES-256-GCM".to_string(),
                wrap_nonce: Some(b64.encode(wrap_nonce)),
                wrapped_dek: b64.encode(&wrapped_dek),
                kms_key_id: None,
            },
            secrets,
            mac,
        };
        let vault_json = serde_json::to_vec_pretty(&vault).unwrap();
        let vectors = serde_json::to_vec_pretty(&serde_json::json!({
            "description": "edgecommons vault v1 cross-language conformance vectors",
            "kekB64": b64.encode(kek),
            "dekB64": b64.encode(dek),
            "vaultId": vault_id,
            "wrapNonceB64": b64.encode(wrap_nonce),
            "wrappedDekB64": b64.encode(&wrapped_dek),
            "macB64": vault.mac,
            "records": record_vectors,
        }))
        .unwrap();

        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vault-test-vectors");
        std::fs::create_dir_all(&dir).unwrap();
        let vault_path = dir.join("vault.json");
        if !vault_path.exists() {
            std::fs::write(&vault_path, &vault_json).unwrap();
            std::fs::write(dir.join("vectors.json"), &vectors).unwrap();
            std::fs::write(dir.join("vault.key"), kek).unwrap();
        }

        // Determinism lock: the committed vault.json must equal a fresh recomputation.
        let on_disk = std::fs::read(&vault_path).unwrap();
        assert_eq!(on_disk, vault_json, "vault.json drifted from the reference computation");

        // The reference implementation must open the committed vault and decrypt it.
        let provider = Arc::new(FileKeyProvider::from_bytes(kek)) as Arc<dyn KeyProvider>;
        let v = LocalVault::open(&vault_path, provider, 2).unwrap();
        assert_eq!(v.get("alpha").unwrap().unwrap().bytes(), b"hello");
        assert_eq!(v.get("beta").unwrap().unwrap().as_json().unwrap()["x"], 1);
    }

    // ===================== env KeyProvider (Phase 1d, FR-CRED-3 / FR-CRED-6) =====================
    // Self-contained: these set unique, per-test env vars and never touch the shared
    // vault-test-vectors/ files (to avoid cross-agent races).

    use crate::credentials::config::build_key_provider;
    use crate::platform::{profile_credentials_key_provider, Platform};

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    /// A `type=env` KeyProviderConfig reading the KEK from `env_var`.
    fn env_kp(env_var: &str) -> KeyProviderConfig {
        KeyProviderConfig {
            kind: Some("env".to_string()),
            env_var: Some(env_var.to_string()),
            ..Default::default()
        }
    }

    // (a) Round-trip through the config path (type=env): create a vault, put a secret, close, reopen
    // with a fresh env provider, get it back.
    #[test]
    fn env_provider_round_trips_via_config_path() {
        let var = "GGTEST_VAULT_KEK_ROUNDTRIP";
        let kek = [7u8; 32];
        unsafe { std::env::set_var(var, b64(&kek)) };

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vault");
        let key_path = format!("{}.key", path.display());

        {
            let provider = build_key_provider(&env_kp(var), &key_path, None).unwrap();
            assert_eq!(provider.provider_id(), "env");
            let c = DefaultCredentialService::new(LocalVault::open(&path, provider, 2).unwrap());
            c.put("db/password", b"s3cr3t", PutOptions::default()).unwrap();
        }
        // Reopen with a fresh env provider built from the same var.
        let provider = build_key_provider(&env_kp(var), &key_path, None).unwrap();
        let c2 = DefaultCredentialService::new(LocalVault::open(&path, provider, 2).unwrap());
        assert_eq!(c2.get_string("db/password").unwrap().unwrap(), "s3cr3t");

        unsafe { std::env::remove_var(var) };
    }

    // (b) Crypto identity with FileKeyProvider given the SAME raw KEK: an env-wrapped vault opens
    // under a FileKeyProvider with K, and provider-level wrap/unwrap interoperates both directions.
    #[test]
    fn env_provider_is_crypto_identical_to_file_provider() {
        let var = "GGTEST_VAULT_KEK_IDENTITY";
        let kek = [7u8; 32];
        unsafe { std::env::set_var(var, b64(&kek)) };

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vault");
        let key_path = format!("{}.key", path.display());

        // Write with the env provider (KekInfo.provider == "env").
        {
            let provider = build_key_provider(&env_kp(var), &key_path, None).unwrap();
            let c = DefaultCredentialService::new(LocalVault::open(&path, provider, 2).unwrap());
            c.put("token", b"abc", PutOptions::default()).unwrap();
        }
        // The on-disk KEK record is tagged `env`.
        let raw = std::fs::read_to_string(&path).unwrap();
        let vf: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(vf["kek"]["provider"], "env");

        // Open with a FileKeyProvider holding the SAME raw KEK — must decrypt (crypto identity).
        let file_provider = Arc::new(FileKeyProvider::from_bytes(kek)) as Arc<dyn KeyProvider>;
        let c2 = DefaultCredentialService::new(LocalVault::open(&path, file_provider, 2).unwrap());
        assert_eq!(c2.get_string("token").unwrap().unwrap(), "abc");

        // Provider-level interop both ways: env-wrap → file-unwrap and file-wrap → env-unwrap.
        let env_p = EnvKeyProvider::from_bytes(kek);
        let file_p = FileKeyProvider::from_bytes(kek);
        assert_eq!(env_p.provider_id(), "env");
        let vault_id = "00000000-0000-4000-8000-0000000000aa";
        let dek = [0x41u8; 32];

        let env_wrapped = env_p.wrap_dek(vault_id, &dek).unwrap();
        assert_eq!(env_wrapped.provider, "env");
        assert_eq!(&file_p.unwrap_dek(vault_id, &env_wrapped).unwrap()[..], &dek[..]);

        let file_wrapped = file_p.wrap_dek(vault_id, &dek).unwrap();
        assert_eq!(file_wrapped.provider, "file");
        assert_eq!(&env_p.unwrap_dek(vault_id, &file_wrapped).unwrap()[..], &dek[..]);

        unsafe { std::env::remove_var(var) };
    }

    // (c) Error cases: unset, empty, invalid base64, wrong length — clear errors, never panics.
    #[test]
    fn env_provider_errors_on_unset_invalid_and_wrong_length() {
        let err = EnvKeyProvider::from_env("GGTEST_VAULT_KEK_DOES_NOT_EXIST_ZZZ").err().unwrap();
        assert!(err.to_string().contains("not set"), "{err}");

        let var_empty = "GGTEST_VAULT_KEK_EMPTY";
        unsafe { std::env::set_var(var_empty, "   ") };
        let err = EnvKeyProvider::from_env(var_empty).err().unwrap();
        assert!(err.to_string().contains("empty"), "{err}");
        unsafe { std::env::remove_var(var_empty) };

        let var_bad = "GGTEST_VAULT_KEK_BADB64";
        unsafe { std::env::set_var(var_bad, "@@@ not valid base64 @@@") };
        let err = EnvKeyProvider::from_env(var_bad).err().unwrap();
        assert!(err.to_string().contains("base64"), "{err}");
        unsafe { std::env::remove_var(var_bad) };

        let var_short = "GGTEST_VAULT_KEK_SHORT";
        unsafe { std::env::set_var(var_short, b64(&[0u8; 16])) }; // valid base64, only 16 bytes
        let err = EnvKeyProvider::from_env(var_short).err().unwrap();
        assert!(err.to_string().contains("32 bytes"), "{err}");
        unsafe { std::env::remove_var(var_short) };
    }

    // The env var NAME defaults to EDGECOMMONS_VAULT_KEK when keyProvider.envVar is absent.
    #[test]
    fn env_provider_defaults_env_var_name_to_edgecommons_vault_kek() {
        assert_eq!(DEFAULT_KEK_ENV_VAR, "EDGECOMMONS_VAULT_KEK");
        unsafe { std::env::set_var(DEFAULT_KEK_ENV_VAR, b64(&[9u8; 32])) };
        let dir = tempfile::tempdir().unwrap();
        let kp = KeyProviderConfig { kind: Some("env".to_string()), env_var: None, ..Default::default() };
        let p = build_key_provider(&kp, &format!("{}.key", dir.path().join("v").display()), None).unwrap();
        assert_eq!(p.provider_id(), "env");
        unsafe { std::env::remove_var(DEFAULT_KEK_ENV_VAR) };
    }

    // (d) Platform default precedence (FR-CRED-6 / FR-RT-3): absent type + KUBERNETES → env;
    // absent + HOST/GREENGRASS → file; an explicit type always wins.
    #[test]
    fn platform_default_selects_env_on_kubernetes_file_elsewhere() {
        let var = "GGTEST_VAULT_KEK_PLATDEFAULT";
        unsafe { std::env::set_var(var, b64(&[7u8; 32])) };
        let dir = tempfile::tempdir().unwrap();

        // Absent type + KUBERNETES default ("env") → env provider.
        let kp_absent_env = KeyProviderConfig { kind: None, env_var: Some(var.to_string()), ..Default::default() };
        let k8s_default = profile_credentials_key_provider(Platform::Kubernetes);
        assert_eq!(k8s_default, Some("env"));
        let p = build_key_provider(&kp_absent_env, &format!("{}.key", dir.path().join("k8s").display()), k8s_default).unwrap();
        assert_eq!(p.provider_id(), "env");

        // Absent type + HOST/GREENGRASS (no profile default) → library default `file`.
        for (label, platform) in [("host", Platform::Host), ("gg", Platform::Greengrass)] {
            let default = profile_credentials_key_provider(platform);
            assert_eq!(default, None);
            let kp_absent = KeyProviderConfig::default(); // kind = None
            let p = build_key_provider(&kp_absent, &format!("{}.key", dir.path().join(label).display()), default).unwrap();
            assert_eq!(p.provider_id(), "file", "{label}");
        }

        // Explicit type ALWAYS wins, even on KUBERNETES (explicit `file` stays `file`).
        let kp_file = KeyProviderConfig { kind: Some("file".to_string()), ..Default::default() };
        let p = build_key_provider(
            &kp_file,
            &format!("{}.key", dir.path().join("explicit").display()),
            profile_credentials_key_provider(Platform::Kubernetes),
        )
        .unwrap();
        assert_eq!(p.provider_id(), "file");

        unsafe { std::env::remove_var(var) };
    }

    // (e) Credentials stay OPT-IN: the KUBERNETES profile default only changes the provider TYPE; it
    // must NOT auto-enable credentials when no `credentials` section is configured. This mirrors the
    // gating in EdgeCommonsBuilder::build (credentials are opened only inside the
    // `snapshot.raw.get("credentials") == Some` arm, where the default is consulted). The full
    // build() path is exercised by the broker-gated integration test `tests/lib_standalone.rs`.
    #[test]
    fn profile_default_does_not_enable_credentials_without_a_section() {
        use crate::config::model::Config;
        // The KUBERNETES profile advertises an "env" default ...
        assert_eq!(profile_credentials_key_provider(Platform::Kubernetes), Some("env"));
        // ... but a config without a `credentials` section means no vault is opened (any platform).
        let cfg = Config::from_value(
            "com.example.C",
            "thing-1",
            serde_json::json!({ "logging": { "level": "INFO" } }),
        )
        .unwrap();
        assert!(cfg.raw.get("credentials").is_none(), "no credentials section present");
        let would_open = cfg.raw.get("credentials").is_some();
        assert!(!would_open, "credentials must stay OFF without a section, even on KUBERNETES");
    }
}
