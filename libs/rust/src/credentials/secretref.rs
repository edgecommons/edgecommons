//! # Secret references (`$secret`) in config
//!
//! **One-liner purpose**: Let any subsystem's config point at a vault secret instead of embedding
//! the value — `{"$secret": "name"}` (whole value) or `{"$secret": "name", "field": "key"}`
//! (a field of the secret's JSON). Resolved lazily at subsystem-init time so the secret never lands
//! in the logged/templated config snapshot. This is how streaming/messaging consume credentials
//! (closes `TELEMETRY_STREAMING.md` §7).

use serde_json::Value;

use super::service::CredentialService;
use crate::error::GgError;
use crate::Result;

/// Recursively replace `$secret` references in `value` with values resolved from `creds`.
///
/// # Errors
/// `GgError::Credentials` if a referenced secret (or requested field) is absent.
pub fn resolve_secret_refs(value: &mut Value, creds: &dyn CredentialService) -> Result<()> {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(name)) = map.get("$secret") {
                let name = name.clone();
                let field = map.get("field").and_then(|v| v.as_str()).map(str::to_string);
                let resolved = resolve_one(&name, field.as_deref(), creds)?;
                *value = Value::String(resolved);
                return Ok(());
            }
            for v in map.values_mut() {
                resolve_secret_refs(v, creds)?;
            }
        }
        Value::Array(arr) => {
            for v in arr {
                resolve_secret_refs(v, creds)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn resolve_one(name: &str, field: Option<&str>, creds: &dyn CredentialService) -> Result<String> {
    let secret = creds
        .get(name)?
        .ok_or_else(|| GgError::Credentials(format!("secretRef '{name}' not found in the vault")))?;
    match field {
        None => Ok(secret.as_str()?.to_string()),
        Some(f) => secret
            .as_json()?
            .get(f)
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| GgError::Credentials(format!("secretRef '{name}' field '{f}' missing or not a string"))),
    }
}
