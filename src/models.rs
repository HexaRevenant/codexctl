use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Credentials parsed from an auth.json file.
#[derive(Debug, Clone)]
pub struct AuthCredentials {
    pub access_token: String,
    pub refresh_token: String,
    pub id_token: Option<String>,
    pub account_id: Option<String>,
    pub last_refresh: Option<DateTime<Utc>>,
    pub email: Option<String>,
    pub name: Option<String>,
    pub plan_type: Option<String>,
    pub user_id: Option<String>,
}

/// A single quota window (5h or 7d).
#[derive(Debug, Clone)]
pub struct UsageWindow {
    pub used_percent: f64,
    pub reset_at: Option<DateTime<Utc>>,
    pub limit_window_seconds: i64,
}

/// An additional quota window identified by the backend-provided metadata.
#[derive(Debug, Clone)]
pub struct AdditionalRateLimit {
    pub limit_name: String,
    pub metered_feature: String,
    pub window: Option<AdditionalRateLimitWindow>,
}

#[derive(Debug, Clone)]
pub struct AdditionalRateLimitWindow {
    pub used_percent: f64,
    pub reset_after_seconds: Option<i64>,
    pub reset_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct FreeResetCredits {
    pub available_count: i64,
    pub applicable_available_count: i64,
}

/// Full quota snapshot from OpenAI.
#[derive(Debug, Clone)]
pub struct QuotaSnapshot {
    pub email: Option<String>,
    pub plan_type: Option<String>,
    pub allowed: Option<bool>,
    pub limit_reached: Option<bool>,
    pub primary_window: Option<UsageWindow>,
    pub secondary_window: Option<UsageWindow>,
    pub credits_balance: Option<f64>,
    pub credits_unlimited: Option<bool>,
    pub rate_limit_reset_credits: Option<FreeResetCredits>,
    pub additional_rate_limits: Vec<AdditionalRateLimit>,
}

/// A managed account stored locally.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub nickname: String,
    pub uuid: String,
    pub email: Option<String>,
    pub plan_type: Option<String>,
    pub provider_account_id: Option<String>,
    #[serde(default)]
    pub user_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}
