use anyhow::{Context, Result};
use base64::Engine;
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::path::Path;

use crate::models::AuthCredentials;

/// Parse an auth.json file and return credentials.
pub fn load(path: &Path) -> Result<AuthCredentials> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("no se pudo leer {}", path.display()))?;
    let raw: Value = serde_json::from_str(&text)
        .with_context(|| format!("JSON inválido en {}", path.display()))?;

    // API-key based
    if let Some(key) = raw.get("OPENAI_API_KEY").and_then(|v| v.as_str()) {
        if !key.is_empty() {
            return Ok(AuthCredentials {
                access_token: key.to_string(),
                refresh_token: String::new(),
                id_token: None,
                account_id: None,
                last_refresh: None,
                email: None,
                name: None,
                plan_type: None,
            });
        }
    }

    // OAuth-token based
    let tokens = raw
        .get("tokens")
        .and_then(|v| v.as_object())
        .context("auth.json no tiene 'tokens'")?;

    let access_token = tokens
        .get("access_token")
        .and_then(|v| v.as_str())
        .context("falta access_token")?;

    let id_token = tokens.get("id_token").and_then(|v| v.as_str());
    let account_id = tokens.get("account_id").and_then(|v| v.as_str());
    let refresh_token = tokens
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let last_refresh = raw
        .get("last_refresh")
        .and_then(|v| v.as_str())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.to_utc());

    let mut creds = AuthCredentials {
        access_token: access_token.to_string(),
        refresh_token: refresh_token.to_string(),
        id_token: id_token.map(String::from),
        account_id: account_id.map(String::from),
        last_refresh,
        email: None,
        name: None,
        plan_type: None,
    };

    // Extract metadata from id_token JWT
    if let Some(id_token) = &creds.id_token {
        if let Some(payload) = decode_jwt(id_token) {
            creds.email = payload
                .get("email")
                .and_then(|v| v.as_str())
                .map(String::from);
            creds.name = payload
                .get("name")
                .and_then(|v| v.as_str())
                .map(String::from);

            if let Some(auth) = payload
                .get("https://api.openai.com/auth")
                .and_then(|v| v.as_object())
            {
                creds.plan_type = creds.plan_type.or_else(|| {
                    auth.get("chatgpt_plan_type")
                        .and_then(|v| v.as_str())
                        .map(String::from)
                });
                creds.account_id = creds.account_id.or_else(|| {
                    auth.get("chatgpt_account_id")
                        .and_then(|v| v.as_str())
                        .map(String::from)
                });
            }
        }
    }

    Ok(creds)
}

/// Write credentials back to auth.json.
pub fn save(creds: &AuthCredentials, path: &Path) -> Result<()> {
    let mut raw: Value = if path.exists() {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        serde_json::from_str(&text).unwrap_or(serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    let mut tokens = serde_json::Map::new();
    tokens.insert("access_token".into(), Value::String(creds.access_token.clone()));
    tokens.insert("refresh_token".into(), Value::String(creds.refresh_token.clone()));
    if let Some(id) = &creds.id_token {
        tokens.insert("id_token".into(), Value::String(id.clone()));
    }
    if let Some(aid) = &creds.account_id {
        tokens.insert("account_id".into(), Value::String(aid.clone()));
    }

    raw["auth_mode"] = Value::String("chatgpt".into());
    raw["tokens"] = Value::Object(tokens);
    raw["last_refresh"] = Value::String(
        Utc::now().format("%Y-%m-%dT%H:%M:%S%.fZ").to_string(),
    );
    raw.as_object_mut().map(|o| o.remove("OPENAI_API_KEY"));

    let text = serde_json::to_string_pretty(&raw)?;
    std::fs::write(path, text + "\n")?;
    Ok(())
}

/// Decode the payload of a JWT without verifying signature.
fn decode_jwt(token: &str) -> Option<serde_json::Value> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() < 2 {
        return None;
    }
    let payload = parts[1];
    // Pad for base64
    let padded = format!("{}{}", payload, "=".repeat((4 - payload.len() % 4) % 4));
    let bytes = base64::engine::general_purpose::URL_SAFE
        .decode(padded.as_bytes())
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}
