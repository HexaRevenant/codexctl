use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde_json::Value;
use std::time::Duration;

use crate::models::{
    AdditionalRateLimit, AdditionalRateLimitWindow, AuthCredentials, FreeResetCredits,
    QuotaSnapshot, UsageWindow,
};

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
        anyhow::bail!(
            "API HTTP {}: {}",
            status,
            text.chars().take(200).collect::<String>()
        );
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
    let rate_limit_reset_credits = data
        .get("rate_limit_reset_credits")
        .and_then(|v| v.as_object())
        .and_then(|credits| {
            Some(FreeResetCredits {
                available_count: integer_value(credits.get("available_count")?)?,
                applicable_available_count: integer_value(
                    credits.get("applicable_available_count")?,
                )?,
            })
        });

    let primary = rate
        .and_then(|r| r.get("primary_window"))
        .and_then(make_window);
    let secondary = rate
        .and_then(|r| r.get("secondary_window"))
        .and_then(make_window);
    let limit_reached = rate
        .and_then(|r| r.get("limit_reached"))
        .and_then(|v| v.as_bool());

    let (primary, secondary) = normalize_windows(primary, secondary);
    let additional_rate_limits = parse_additional_rate_limits(data);

    Ok(QuotaSnapshot {
        email: creds.email.clone(),
        plan_type: data
            .get("plan_type")
            .and_then(|v| v.as_str())
            .or(creds.plan_type.as_deref())
            .map(String::from),
        allowed: rate
            .and_then(|r| r.get("allowed"))
            .and_then(|v| v.as_bool()),
        limit_reached,
        primary_window: primary,
        secondary_window: secondary,
        credits_balance: credits
            .and_then(|c| c.get("balance"))
            .and_then(|v| v.as_f64()),
        credits_unlimited: credits
            .and_then(|c| c.get("unlimited"))
            .and_then(|v| v.as_bool()),
        rate_limit_reset_credits,
        additional_rate_limits,
    })
}

fn parse_additional_rate_limits(data: &Value) -> Vec<AdditionalRateLimit> {
    data.get("additional_rate_limits")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|raw| {
            let obj = raw.as_object()?;
            let limit_name = obj.get("limit_name")?.as_str()?.to_string();
            let metered_feature = obj.get("metered_feature")?.as_str()?.to_string();
            let window =
                obj.get("rate_limit")
                    .and_then(|v| v.as_object())
                    .and_then(|rate| {
                        Some(AdditionalRateLimitWindow {
                            used_percent: number_value(rate.get("used_percent")?)?,
                            reset_after_seconds: rate
                                .get("reset_after_seconds")
                                .and_then(integer_value),
                            reset_at: rate.get("reset_at").and_then(number_value).and_then(
                                |timestamp| DateTime::from_timestamp(timestamp as i64, 0),
                            ),
                        })
                    });
            Some(AdditionalRateLimit {
                limit_name,
                metered_feature,
                window,
            })
        })
        .collect()
}

fn make_window(raw: &Value) -> Option<UsageWindow> {
    let obj = raw.as_object()?;
    // `reset_at` is not required to calculate/display the quota. Some API
    // responses omit it or return null while the window is still valid.
    let used = number_value(obj.get("used_percent")?)?;
    let reset = obj.get("reset_at").and_then(number_value);
    let seconds = obj.get("limit_window_seconds")?.as_i64()?;
    Some(UsageWindow {
        used_percent: used,
        reset_at: reset.and_then(|timestamp| DateTime::from_timestamp(timestamp as i64, 0)),
        limit_window_seconds: seconds,
    })
}

fn number_value(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|text| text.parse::<f64>().ok()))
}

fn integer_value(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|text| text.parse::<i64>().ok()))
}

fn normalize_windows(
    p: Option<UsageWindow>,
    s: Option<UsageWindow>,
) -> (Option<UsageWindow>, Option<UsageWindow>) {
    match (&p, &s) {
        (Some(pw), Some(sw)) => {
            let p_role = window_role(pw.limit_window_seconds);
            let s_role = window_role(sw.limit_window_seconds);
            if matches!(
                (p_role, s_role),
                ("weekly", "session") | ("weekly", "unknown")
            ) {
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

#[cfg(test)]
mod tests {
    use super::{make_window, parse_snapshot};
    use crate::models::AuthCredentials;
    use serde_json::json;

    fn credentials() -> AuthCredentials {
        AuthCredentials {
            access_token: String::new(),
            refresh_token: String::new(),
            id_token: None,
            account_id: None,
            last_refresh: None,
            email: None,
            name: None,
            plan_type: None,
            user_id: None,
        }
    }

    #[test]
    fn parses_quota_when_reset_at_is_missing() {
        let window = make_window(&json!({
            "used_percent": 37.5,
            "limit_window_seconds": 18_000
        }))
        .expect("quota window should still be usable");

        assert_eq!(window.used_percent, 37.5);
        assert!(window.reset_at.is_none());
    }

    #[test]
    fn accepts_string_percentage_and_reset_timestamp() {
        let window = make_window(&json!({
            "used_percent": "42.0",
            "reset_at": "1700000000",
            "limit_window_seconds": 604_800
        }))
        .expect("quota window should parse");

        assert_eq!(window.used_percent, 42.0);
        assert!(window.reset_at.is_some());
    }

    #[test]
    fn parses_additional_rate_limits_fixture_without_changing_normal_windows() {
        let data: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/additional_rate_limits.json"
        ))
        .expect("fixture should be valid JSON");

        let snapshot = parse_snapshot(&data, &credentials()).expect("snapshot should parse");
        assert_eq!(snapshot.primary_window.as_ref().unwrap().used_percent, 12.0);
        assert_eq!(
            snapshot.secondary_window.as_ref().unwrap().used_percent,
            34.0
        );
        assert_eq!(snapshot.additional_rate_limits.len(), 2);
        assert_eq!(snapshot.additional_rate_limits[0].limit_name, "reviews");
        assert_eq!(
            snapshot.additional_rate_limits[0].metered_feature,
            "codex_reviews"
        );
        assert_eq!(
            snapshot.additional_rate_limits[0]
                .window
                .as_ref()
                .unwrap()
                .used_percent,
            50.0
        );
        assert_eq!(
            snapshot.additional_rate_limits[0]
                .window
                .as_ref()
                .unwrap()
                .reset_after_seconds,
            Some(3600)
        );
        assert!(snapshot.additional_rate_limits[0]
            .window
            .as_ref()
            .unwrap()
            .reset_at
            .is_some());
        assert!(snapshot.additional_rate_limits[1].window.is_none());
    }

    #[test]
    fn parses_free_reset_credits_fixture() {
        let data: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/free_reset_credits.json"))
                .expect("fixture should be valid JSON");

        let snapshot = parse_snapshot(&data, &credentials()).expect("snapshot should parse");
        let resets = snapshot
            .rate_limit_reset_credits
            .expect("free reset credits should parse");
        assert_eq!(resets.available_count, 5);
        assert_eq!(resets.applicable_available_count, 3);
    }

    #[test]
    fn leaves_free_reset_credits_absent_when_provider_omits_them() {
        let snapshot =
            parse_snapshot(&serde_json::json!({}), &credentials()).expect("snapshot should parse");

        assert!(snapshot.rate_limit_reset_credits.is_none());
    }
}
