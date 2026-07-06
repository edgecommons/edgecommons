//! Central-sync integration test: bootstrap + refresh + rotation from AWS Secrets Manager,
//! exercised against the local floci emulator (port 4566).
//!
//! Gated: no-op unless `EDGECOMMONS_IT_SM=1` is set (floci must be running with secretsmanager).

#![cfg(feature = "credentials-aws")]

use edgecommons::credentials::{self, CredentialService, CredentialsConfig};

/// POST a Secrets Manager JSON request to floci via curl (avoids adding the SDK as a dev-dep).
fn sm(target: &str, body: &str) {
    let out = std::process::Command::new("curl")
        .args([
            "-s",
            "-m",
            "10",
            "-X",
            "POST",
            "http://localhost:4566/",
            "-H",
            &format!("X-Amz-Target: {target}"),
            "-H",
            "Content-Type:application/x-amz-json-1.1",
            "-H",
            "Authorization: x",
            "-d",
            body,
        ])
        .output()
        .expect("curl available");
    assert!(out.status.success(), "floci request {target} failed");
}

#[test]
fn bootstrap_refresh_and_rotation_from_secrets_manager() {
    if std::env::var("EDGECOMMONS_IT_SM").is_err() {
        eprintln!("skipping Secrets Manager sync test (set EDGECOMMONS_IT_SM=1 + run floci)");
        return;
    }
    // Dummy creds so the AWS SDK signs requests to floci (which ignores them).
    unsafe {
        std::env::set_var("AWS_ACCESS_KEY_ID", "test");
        std::env::set_var("AWS_SECRET_ACCESS_KEY", "test");
        std::env::set_var("AWS_REGION", "us-east-1");
    }

    let name = format!("edgecommons-cred-it-{}", uuid::Uuid::new_v4());
    sm("secretsmanager.CreateSecret", &format!(r#"{{"Name":"{name}","SecretString":"v1"}}"#));

    let dir = std::env::temp_dir().join(format!("ggcred-{}", uuid::Uuid::new_v4()));
    let vault_path = dir.join("vault");
    let cfg: CredentialsConfig = serde_json::from_value(serde_json::json!({
        "vault": {
            "path": vault_path.to_string_lossy(),
            "keyProvider": { "type": "file", "keyPath": dir.join("vault.key").to_string_lossy() }
        },
        "central": {
            "type": "awsSecretsManager",
            "region": "us-east-1",
            "endpointUrl": "http://localhost:4566",
            "bootstrapOnStart": true,
            "refreshIntervalSecs": 0,
            "sync": { "secrets": [name.clone()] }
        }
    }))
    .unwrap();

    let creds = credentials::open(&cfg).expect("open vault + bootstrap from central");

    // Bootstrap pulled the secret into the local vault.
    assert_eq!(creds.get_string(&name).unwrap().unwrap(), "v1");
    assert_eq!(creds.list("").unwrap().len(), 1);

    // Rotate upstream, refresh, and confirm the new value — with the previous version retained.
    sm("secretsmanager.PutSecretValue", &format!(r#"{{"SecretId":"{name}","SecretString":"v2"}}"#));
    creds.refresh().unwrap();
    assert_eq!(creds.get_string(&name).unwrap().unwrap(), "v2");
    assert!(creds.versions(&name).unwrap().len() >= 2, "previous version retained for rotation grace");

    // Idempotency: a refresh with no upstream change adds no new version.
    let before = creds.versions(&name).unwrap().len();
    creds.refresh().unwrap();
    assert_eq!(creds.versions(&name).unwrap().len(), before, "no churn when unchanged");

    let _ = std::fs::remove_dir_all(&dir);
    sm("secretsmanager.DeleteSecret", &format!(r#"{{"SecretId":"{name}","ForceDeleteWithoutRecovery":true}}"#));
}

/// POST to floci and return the response body.
fn floci_out(target: &str, body: &str) -> String {
    let out = std::process::Command::new("curl")
        .args([
            "-s", "-m", "10", "-X", "POST", "http://localhost:4566/",
            "-H", &format!("X-Amz-Target: {target}"),
            "-H", "Content-Type:application/x-amz-json-1.1",
            "-H", "Authorization: x", "-d", body,
        ])
        .output()
        .expect("curl available");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn kms_key_provider_round_trip() {
    if std::env::var("EDGECOMMONS_IT_SM").is_err() {
        eprintln!("skipping KMS key-provider test (set EDGECOMMONS_IT_SM=1 + run floci)");
        return;
    }
    unsafe {
        std::env::set_var("AWS_ACCESS_KEY_ID", "test");
        std::env::set_var("AWS_SECRET_ACCESS_KEY", "test");
        std::env::set_var("AWS_REGION", "us-east-1");
    }
    // Create a CMK in floci (KMS service target prefix is "TrentService").
    let created = floci_out("TrentService.CreateKey", "{}");
    let v: serde_json::Value = serde_json::from_str(&created).expect("CreateKey JSON");
    let key_id = v["KeyMetadata"]["KeyId"].as_str().expect("KeyId").to_string();

    let dir = std::env::temp_dir().join(format!("ggcred-kms-{}", uuid::Uuid::new_v4()));
    let cfg: CredentialsConfig = serde_json::from_value(serde_json::json!({
        "vault": {
            "path": dir.join("vault").to_string_lossy(),
            "keyProvider": {
                "type": "kms", "kmsKeyId": key_id, "region": "us-east-1",
                "endpointUrl": "http://localhost:4566"
            }
        }
    }))
    .unwrap();

    // Open with the KMS-wrapped DEK, write a secret, then reopen — proving the DEK round-trips
    // through KMS encrypt/decrypt.
    {
        let creds = credentials::open(&cfg).expect("open KMS-backed vault");
        creds.put("k", b"v", Default::default()).unwrap();
        assert_eq!(creds.get_string("k").unwrap().unwrap(), "v");
    }
    let creds2 = credentials::open(&cfg).expect("reopen KMS-backed vault");
    assert_eq!(creds2.get_string("k").unwrap().unwrap(), "v");

    let _ = std::fs::remove_dir_all(&dir);
}
