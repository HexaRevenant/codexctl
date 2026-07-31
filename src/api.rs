use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde_json::Value;
use std::time::Duration;

use crate::models::{AuthCredentials, QuotaSnapshot, UsageWindow};

const REFRESH_ENDPOINT: &str = "https://auth.openai.com/oauth/token";
const USAGE_DEFAULT_BASE: &str = "https://chatgpt.com/backend-api";
const REFRESH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const REQUEST_TIMEOUT: u64 = 30;
const UNAUTHORIZED_MSG: &str = "API returned 401/403";

/// Fetch live quota from OpenAI.
pub async fn fetch_quota(client: &Client, creds: &AuthCredentials) -> Result<QuotaSnapshot> {
    let url = format!("{}/wham/usage", USAGE_DEFAULT_BASE);
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "Authorization",
        format!("Bearer {}", creds.access_token).parse().unwrap(),
    );
    headers.insert("User-Agent", "codex-cli".parse().unwrap());
    headers.insert("Accept", "application/json".parse().unwrap());
    headers.insert("Cache-Control", "no-cache".parse().unwrap());
    if let Some(aid) = &creds.account_id {
        headers.insert("ChatGPT-Account-Id", aid.parse().unwrap());
    }

    let resp = client
        .get(&url)
        .headers(headers)
        .timeout(Duration::from_secs(REQUEST_TIMEOUT))
        .send()
        .await
        .context("error de red al consultar quota")?;

    let status = resp.status();
    if status == 401 || status == 403 {
        anyhow::bail!(UNAUTHORIZED_MSG);
    }
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("API HTTP {}: {}", status, text.chars().take(200).collect::<String>());
    }

    let data: Value = resp.json().await.context("API response JSON inválido")?;
    parse_snapshot(&data, creds)
}

/// Try to refresh tokens if stale (> 8 days).
pub async fn maybe_refresh(
    client: &Client,
    creds: &AuthCredentials,
) -> Result<Option<AuthCredentials>> {
    if let Some(last) = &creds.last_refresh {
        let age = Utc::now() - *last;
        if age.num_days() < 8 {
            return Ok(None);
        }
    }
    if creds.refresh_token.is_empty() {
        return Ok(None);
    }
    do_refresh(client, creds).await.map(Some)
}

/// Force token refresh.
pub async fn force_refresh(client: &Client, creds: &AuthCredentials) -> Result<AuthCredentials> {
    if creds.refresh_token.is_empty() {
        anyhow::bail!("no hay refresh_token disponible");
    }
    do_refresh(client, creds).await
}

async fn do_refresh(client: &Client, creds: &AuthCredentials) -> Result<AuthCredentials> {
    let body = serde_json::json!({
        "client_id": REFRESH_CLIENT_ID,
        "grant_type": "refresh_token",
        "refresh_token": creds.refresh_token,
        "scope": "openid profile email",
    });
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("Content-Type", "application/json".parse().unwrap());
    headers.insert("Cache-Control", "no-cache".parse().unwrap());

    let resp = client
        .post(REFRESH_ENDPOINT)
        .headers(headers)
        .json(&body)
        .timeout(Duration::from_secs(REQUEST_TIMEOUT))
        .send()
        .await
        .context("error de red al refrescar token")?;

    let status = resp.status();
    if status == 401 {
        let text = resp.text().await.unwrap_or_default();
        if text.contains("reused") {
            anyhow::bail!("refresh_token reusado — iniciá sesión de nuevo");
        }
        if text.contains("invalidated") {
            anyhow::bail!("refresh_token revocado — iniciá sesión de nuevo");
        }
        anyhow::bail!("refresh_token expirado — iniciá sesión de nuevo");
    }
    if !status.is_success() {
        anyhow::bail!("token refresh HTTP {}", status);
    }

    let data: Value = resp.json().await?;

    let new_access = data
        .get("access_token")
        .and_then(|v| v.as_str())
        .unwrap_or(&creds.access_token);
    let new_refresh = data
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .unwrap_or(&creds.refresh_token);

    Ok(AuthCredentials {
        access_token: new_access.to_string(),
        refresh_token: new_refresh.to_string(),
        id_token: data
            .get("id_token")
            .and_then(|v| v.as_str())
            .or(creds.id_token.as_deref())
            .map(String::from),
        account_id: creds.account_id.clone(),
        last_refresh: Some(Utc::now()),
        email: creds.email.clone(),
        name: creds.name.clone(),
        plan_type: creds.plan_type.clone(),
        user_id: creds.user_id.clone(),
    })
}

fn parse_snapshot(data: &Value, creds: &AuthCredentials) -> Result<QuotaSnapshot> {
    let rate = data.get("rate_limit").and_then(|v| v.as_object());
    let credits = data.get("credits").and_then(|v| v.as_object());

    let primary = rate
        .and_then(|r| r.get("primary_window"))
        .and_then(make_window);
    let secondary = rate
        .and_then(|r| r.get("secondary_window"))
        .and_then(make_window);
    let limit_reached = rate.and_then(|r| r.get("limit_reached")).and_then(|v| v.as_bool());

    let (primary, secondary) = normalize_windows(primary, secondary);

    Ok(QuotaSnapshot {
        email: creds.email.clone(),
        plan_type: data
            .get("plan_type")
            .and_then(|v| v.as_str())
            .or(creds.plan_type.as_deref())
            .map(String::from),
        allowed: rate.and_then(|r| r.get("allowed")).and_then(|v| v.as_bool()),
        limit_reached,
        primary_window: primary,
        secondary_window: secondary,
        credits_balance: credits
            .and_then(|c| c.get("balance"))
            .and_then(|v| v.as_f64()),
        credits_unlimited: credits
            .and_then(|c| c.get("unlimited"))
            .and_then(|v| v.as_bool()),
    })
}

fn make_window(raw: &Value) -> Option<UsageWindow> {
    let obj = raw.as_object()?;
    let used = obj.get("used_percent")?.as_f64()?;
    let reset = obj.get("reset_at")?.as_f64()?;
    let seconds = obj.get("limit_window_seconds")?.as_i64()?;
    Some(UsageWindow {
        used_percent: used,
        reset_at: DateTime::from_timestamp(reset as i64, 0),
        limit_window_seconds: seconds,
    })
}

fn normalize_windows(
    p: Option<UsageWindow>,
    s: Option<UsageWindow>,
) -> (Option<UsageWindow>, Option<UsageWindow>) {
    match (&p, &s) {
        (Some(pw), Some(sw)) => {
            let p_role = window_role(pw.limit_window_seconds);
            let s_role = window_role(sw.limit_window_seconds);
            if matches!((p_role, s_role), ("weekly", "session") | ("weekly", "unknown")) {
                (s, p)
            } else {
                (p, s)
            }
        }
        (Some(pw), None) => {
            if window_role(pw.limit_window_seconds) == "weekly" {
                (None, p)
            } else {
                (p, None)
            }
        }
        (None, Some(sw)) => {
            if window_role(sw.limit_window_seconds) == "session" {
                (s, None)
            } else {
                (None, s)
            }
        }
        (None, None) => (None, None),
    }
}

fn window_role(seconds: i64) -> &'static str {
    match seconds {
        18_000 => "session",
        604_800 => "weekly",
        _ => "unknown",
    }
}
