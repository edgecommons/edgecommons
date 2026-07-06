//! Integration coverage for the **local** credentials surface — the in-process, CI-reachable paths
//! of `gg.credentials()`: the public [`CredentialService`] (incl. the convenience + typed-view
//! accessors), `$secret` config references, access auditing, and the `LocalVault` on-disk store.
//!
//! Everything here runs against a temp-dir file-key-provider vault — no AWS, HSM, or central sync
//! (those are validated on the lab/floci and are out of the gate).

#![cfg(feature = "credentials")]

use std::sync::Arc;

use edgecommons::credentials::{
    self, CentralConfig, CredentialService, CredentialsConfig, FileKeyProvider, KeyProvider,
    LocalVault, PutOptions, VaultConfig,
};

fn open_service(dir: &std::path::Path) -> credentials::DefaultCredentialService {
    let cfg = CredentialsConfig {
        vault: VaultConfig {
            path: dir.join("vault").to_string_lossy().into_owned(),
            ..VaultConfig::default()
        },
        // central.type defaults to "none" → a standalone local vault; audit on by default.
        central: CentralConfig::default(),
        ..CredentialsConfig::default()
    };
    credentials::open(&cfg).expect("open a local file-key-provider vault")
}

#[test]
fn put_get_and_the_convenience_value_accessors() {
    let dir = tempfile::tempdir().unwrap();
    let svc = open_service(dir.path());

    svc.put("svc/token", b"top-secret", PutOptions::default())
        .unwrap();

    // bytes / string / json convenience accessors all see the same value.
    assert_eq!(
        svc.get_bytes("svc/token").unwrap().unwrap().as_slice(),
        b"top-secret"
    );
    assert_eq!(svc.get_string("svc/token").unwrap().unwrap(), "top-secret");

    svc.put("svc/json", br#"{"a":1,"b":"two"}"#, PutOptions::default())
        .unwrap();
    let v = svc.get_json("svc/json").unwrap().unwrap();
    assert_eq!(v["a"], 1);
    assert_eq!(v["b"], "two");

    // Misses are None across every accessor (not errors).
    assert!(svc.get("nope").unwrap().is_none());
    assert!(svc.get_string("nope").unwrap().is_none());
    assert!(svc.get_bytes("nope").unwrap().is_none());
    assert!(svc.get_json("nope").unwrap().is_none());
    assert!(!svc.exists("nope").unwrap());
    assert!(svc.exists("svc/token").unwrap());
}

#[test]
fn versions_list_get_version_and_delete() {
    let dir = tempfile::tempdir().unwrap();
    let svc = open_service(dir.path());

    // keep_versions defaults to 2, so two writes keep both versions.
    let v1 = svc.put("rotating", b"one", PutOptions::default()).unwrap();
    let v2 = svc.put("rotating", b"two", PutOptions::default()).unwrap();
    assert_ne!(v1, v2);

    let versions = svc.versions("rotating").unwrap();
    assert_eq!(versions, vec![v1.clone(), v2.clone()], "oldest→newest");

    // The latest is "two"; an explicit older version still reads "one".
    assert_eq!(svc.get("rotating").unwrap().unwrap().bytes(), b"two");
    assert_eq!(
        svc.get_version("rotating", &v1).unwrap().unwrap().bytes(),
        b"one"
    );
    assert!(svc.get_version("rotating", "99999999").unwrap().is_none());

    // list returns metadata only (no value) for each secret.
    let metas = svc.list("rotat").unwrap();
    assert_eq!(metas.len(), 1);
    assert_eq!(metas[0].name, "rotating");

    assert!(
        svc.delete("rotating").unwrap(),
        "deleting an existing secret returns true"
    );
    assert!(
        !svc.delete("rotating").unwrap(),
        "deleting a missing secret returns false"
    );
    assert!(svc.get("rotating").unwrap().is_none());

    // refresh is a no-op without a central source (and must not error); stats reports the count.
    svc.refresh().unwrap();
    svc.put("a", b"1", PutOptions::default()).unwrap();
    svc.put("b", b"2", PutOptions::default()).unwrap();
    let stats = svc.stats();
    assert_eq!(stats.secret_count, 2);
    assert!(
        stats.last_sync_age_ms.is_none(),
        "no central sync configured"
    );
}

#[test]
fn typed_views_parse_well_known_json_shapes() {
    let dir = tempfile::tempdir().unwrap();
    let svc = open_service(dir.path());

    svc.put(
        "aws",
        br#"{"accessKeyId":"AKIA","secretAccessKey":"sk","sessionToken":"tok"}"#,
        PutOptions::default(),
    )
    .unwrap();
    let aws = svc.get_aws_credentials("aws").unwrap().unwrap();
    assert_eq!(aws.access_key_id, "AKIA");
    assert_eq!(aws.secret_access_key, "sk");
    assert_eq!(aws.session_token.as_deref(), Some("tok"));

    svc.put(
        "basic",
        br#"{"username":"u","password":"p"}"#,
        PutOptions::default(),
    )
    .unwrap();
    let basic = svc.get_basic_auth("basic").unwrap().unwrap();
    assert_eq!(
        (basic.username.as_str(), basic.password.as_str()),
        ("u", "p")
    );

    svc.put(
        "tls",
        br#"{"certPem":"CERT","keyPem":"KEY"}"#,
        PutOptions::default(),
    )
    .unwrap();
    let tls = svc.get_tls_bundle("tls").unwrap().unwrap();
    assert_eq!(tls.cert_pem, "CERT");
    assert!(tls.ca_pem.is_none());

    svc.put(
        "kafka",
        br#"{"username":"ku","password":"kp"}"#,
        PutOptions::default(),
    )
    .unwrap();
    let kafka = svc.get_kafka_sasl("kafka").unwrap().unwrap();
    assert_eq!(kafka.mechanism, "PLAIN", "mechanism defaults to PLAIN");
    assert_eq!(kafka.username, "ku");

    // A typed view over malformed JSON is a (non-sensitive) error, not a panic.
    svc.put("bad", b"not json", PutOptions::default()).unwrap();
    assert!(svc.get_basic_auth("bad").is_err());

    // Typed views of a missing secret are None.
    assert!(svc.get_aws_credentials("absent").unwrap().is_none());
}

#[test]
fn secret_debug_redacts_the_value() {
    let dir = tempfile::tempdir().unwrap();
    let svc = open_service(dir.path());
    svc.put("pw", b"do-not-leak", PutOptions::default())
        .unwrap();
    let secret = svc.get("pw").unwrap().unwrap();
    let rendered = format!("{secret:?}");
    assert!(
        rendered.contains("redacted"),
        "Debug must redact: {rendered}"
    );
    assert!(
        !rendered.contains("do-not-leak"),
        "the value must never appear in Debug: {rendered}"
    );
}

#[test]
fn audit_default_log_sink_records_access_without_panicking() {
    // open() leaves audit on by default → the default LogAuditSink fires on get/put/delete. We can't
    // capture the tracing record here, but exercising it proves the wiring + sink don't panic.
    let dir = tempfile::tempdir().unwrap();
    let svc = open_service(dir.path());
    svc.put("k", b"v", PutOptions::default()).unwrap();
    assert!(svc.get("k").unwrap().is_some());
    assert!(svc.get("missing-audit").unwrap().is_none()); // a "miss" audit event
    assert!(svc.delete("k").unwrap());
}

#[test]
fn secret_refs_in_config_resolve_from_the_vault() {
    let dir = tempfile::tempdir().unwrap();
    let svc = open_service(dir.path());
    svc.put("db/password", b"hunter2", PutOptions::default())
        .unwrap();
    svc.put("blob", br#"{"token":"abc123"}"#, PutOptions::default())
        .unwrap();

    // Whole-value ref, a field ref, and refs nested inside an array (exercises every branch).
    let mut config = serde_json::json!({
        "password": { "$secret": "db/password" },
        "items": [
            { "token": { "$secret": "blob", "field": "token" } },
            { "plain": "left-alone" }
        ]
    });
    credentials::resolve_secret_refs(&mut config, &svc).unwrap();

    assert_eq!(config["password"], "hunter2");
    assert_eq!(config["items"][0]["token"], "abc123");
    assert_eq!(config["items"][1]["plain"], "left-alone");

    // A reference to a missing secret is a hard error.
    let mut missing = serde_json::json!({ "x": { "$secret": "no-such-secret" } });
    assert!(credentials::resolve_secret_refs(&mut missing, &svc).is_err());
}

#[test]
fn vault_id_and_latest_central_version_id_and_format_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let svc = open_service(dir.path());

    // vault_id() is a stable non-empty id; latest_central_version_id reflects a synced version tag.
    let vault_arc = svc.vault_arc();
    {
        let mut v = vault_arc.lock().unwrap();
        assert!(!v.vault_id().is_empty());
        v.put(
            "synced",
            b"value",
            PutOptions {
                central_version_id: Some("upstream-7".to_string()),
                ..PutOptions::default()
            },
        )
        .unwrap();
        assert_eq!(
            v.latest_central_version_id("synced").as_deref(),
            Some("upstream-7")
        );
        assert!(v.latest_central_version_id("absent").is_none());
    }

    // Opening a vault file with an unsupported format version fails closed.
    let dir2 = tempfile::tempdir().unwrap();
    let path = dir2.path().join("v2");
    let provider = Arc::new(FileKeyProvider::from_bytes([5u8; 32])) as Arc<dyn KeyProvider>;
    {
        let mut v = LocalVault::open(&path, provider.clone(), 2).unwrap();
        v.put("x", b"y", PutOptions::default()).unwrap();
    }
    let mut json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    json["format"] = serde_json::json!(99999);
    std::fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
    assert!(
        LocalVault::open(&path, provider, 2).is_err(),
        "an unsupported on-disk format must be rejected"
    );
}
