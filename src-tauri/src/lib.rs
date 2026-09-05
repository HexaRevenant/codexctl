use serde::Serialize;
use std::time::Duration;

#[derive(Serialize)]
struct AdditionalRateLimitInfo {
    limit_name: String,
    metered_feature: String,
    used_percent: String,
    reset_after_seconds: Option<i64>,
    reset_at: String,
}

#[derive(Serialize)]
struct FreeResetCreditsInfo {
    available_count: i64,
    applicable_available_count: i64,
}

#[derive(Serialize)]
struct AccountInfo {
    id: String,
    nickname: String,
    email: String,
    plan_type: String,
    is_active: bool,
    active_in: String,
    quota_5h: String,
    disp_5h: String,
    quota_7d: String,
    disp_7d: String,
    reset_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    rate_limit_reset_credits: Option<FreeResetCreditsInfo>,
    additional_rate_limits: Vec<AdditionalRateLimitInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    quota_error: Option<String>,
}

type QuotaValues = (
    String,
    String,
    String,
    String,
    String,
    Option<FreeResetCreditsInfo>,
    Vec<AdditionalRateLimitInfo>,
);

#[tauri::command]
async fn list_accounts(fetch_quota: bool) -> Result<Vec<AccountInfo>, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let accounts = codexctl::manager::load().map_err(|e| e.to_string())?;
    let active = codexctl::manager::active(&accounts);
    let targets = codexctl::manager::active_targets(&accounts);

    let quotas = if fetch_quota {
        let mut tasks = tokio::task::JoinSet::new();
        for (index, acct) in accounts.iter().enumerate() {
            let client = client.clone();
            let auth_path = codexctl::manager::homes_dir()
                .join(&acct.uuid)
                .join("auth.json");
            tasks.spawn(async move { (index, quota_for_account(&client, &auth_path).await) });
        }

        let mut quotas = std::iter::repeat_with(|| None)
            .take(accounts.len())
            .collect::<Vec<_>>();
        while let Some(task) = tasks.join_next().await {
            if let Ok((index, quota)) = task {
                quotas[index] = Some(quota);
            }
        }
        quotas
    } else {
        std::iter::repeat_with(|| None)
            .take(accounts.len())
            .collect()
    };

    let mut result = Vec::with_capacity(accounts.len());
    for (acct, quota) in accounts.iter().zip(quotas) {
        let is_active = active.as_ref().map(|a| a.id == acct.id).unwrap_or(false);
        let active_in = targets.get(&acct.id).cloned().unwrap_or_default();
        let (values, quota_error) = match quota {
            Some(Ok(values)) => (values, None),
            Some(Err(error)) => (
                (
                    "—".into(),
                    "—".into(),
                    "—".into(),
                    "—".into(),
                    "—".into(),
                    None,
                    Vec::new(),
                ),
                Some(error),
            ),
            None => (
                (
                    "—".into(),
                    "—".into(),
                    "—".into(),
                    "—".into(),
                    "—".into(),
                    None,
                    Vec::new(),
                ),
                None,
            ),
        };
        let (p5, d5, p7, d7, reset, free_reset_credits, additional) = values;
        result.push(AccountInfo {
            id: acct.id.clone(),
            nickname: acct.nickname.clone(),
            email: acct.email.clone().unwrap_or_default(),
            plan_type: acct.plan_type.clone().unwrap_or_default(),
            is_active,
            active_in,
            quota_5h: p5,
            disp_5h: d5,
            quota_7d: p7,
            disp_7d: d7,
            reset_at: reset,
            rate_limit_reset_credits: free_reset_credits,
            additional_rate_limits: additional,
            quota_error,
        });
    }
    Ok(result)
}

#[tauri::command]
async fn switch_account(account_id: String) -> Result<String, String> {
    let acct = codexctl::manager::switch(&account_id).map_err(|e| e.to_string())?;
    Ok(format!("✅ Activada: {}", acct.nickname))
}

/// Switch account to Codex, OpenCode, or both.
/// targets is a subset of ["codex", "opencode"].
#[tauri::command]
async fn switch_account_targets(
    account_id: String,
    targets: Vec<String>,
) -> Result<String, String> {
    let mut done: Vec<String> = Vec::new();

    if targets.iter().any(|t| t == "codex") || targets.is_empty() {
        let acct = codexctl::manager::switch(&account_id).map_err(|e| e.to_string())?;
        done.push(format!("Codex → {}", acct.nickname));
    }
    if targets.iter().any(|t| t == "opencode") {
        let client = reqwest::Client::new();
        let acct = codexctl::manager::opencode_switch(&client, &account_id)
            .await
            .map_err(|e| e.to_string())?;
        done.push(format!("OpenCode → {}", acct.nickname));
    }

    Ok(format!("✅ {}", done.join(" · ")))
}

#[tauri::command]
async fn rename_account(account_id: String, nickname: String) -> Result<String, String> {
    codexctl::manager::rename(&account_id, &nickname).map_err(|e| e.to_string())?;
    Ok(format!("✅ Renombrada a \"{nickname}\""))
}

#[tauri::command]
async fn remove_account(account_id: String) -> Result<String, String> {
    codexctl::manager::remove(&account_id).map_err(|e| e.to_string())?;
    Ok("✅ Eliminada".into())
}

#[tauri::command]
async fn get_status() -> Result<serde_json::Value, String> {
    let accounts = codexctl::manager::load().map_err(|e| e.to_string())?;
    match codexctl::manager::active(&accounts) {
        Some(a) => Ok(serde_json::json!({
            "id": a.id, "nickname": a.nickname,
            "email": a.email, "plan_type": a.plan_type,
        })),
        None => Ok(serde_json::json!(null)),
    }
}

#[tauri::command]
fn get_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[tauri::command]
async fn add_account(nickname: String) -> Result<String, String> {
    let acct = codexctl::manager::add(&nickname).map_err(|e| e.to_string())?;
    Ok(format!(
        "✅ Agregada: \"{}\" (ID: {})",
        acct.nickname, acct.id
    ))
}

#[tauri::command]
async fn reauth_account(account_id: String) -> Result<String, String> {
    let acct = codexctl::manager::reauth(&account_id).map_err(|e| e.to_string())?;
    Ok(format!("✅ Reautenticada: {}", acct.nickname))
}

/// Export all accounts (auth.json content included) to a single JSON file.
#[tauri::command]
async fn export_accounts(path: String) -> Result<String, String> {
    let accounts = codexctl::manager::load().map_err(|e| e.to_string())?;
    let homes = codexctl::manager::homes_dir();

    let mut items: Vec<serde_json::Value> = Vec::new();
    for acct in &accounts {
        let auth_path = homes.join(&acct.uuid).join("auth.json");
        let auth: serde_json::Value = if auth_path.exists() {
            std::fs::read_to_string(&auth_path)
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or(serde_json::Value::Null)
        } else {
            serde_json::Value::Null
        };

        items.push(serde_json::json!({
            "id": acct.id,
            "nickname": acct.nickname,
            "uuid": acct.uuid,
            "email": acct.email,
            "plan_type": acct.plan_type,
            "provider_account_id": acct.provider_account_id,
            "user_id": acct.user_id,
            "created_at": acct.created_at,
            "updated_at": acct.updated_at,
            "auth": auth,
        }));
    }

    let export = serde_json::json!({
        "app": "codexctl",
        "format": 1,
        "exported_at": chrono::Utc::now().to_rfc3339(),
        "accounts": items,
    });

    std::fs::write(
        &path,
        serde_json::to_string_pretty(&export).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("no se pudo escribir: {e}"))?;

    Ok(format!(
        "✅ Exportadas {} cuentas a {}",
        accounts.len(),
        path
    ))
}

/// Import accounts from an exported JSON file (skips already-known uuids).
#[tauri::command]
async fn import_accounts(path: String) -> Result<String, String> {
    let content = std::fs::read_to_string(&path).map_err(|e| format!("no se pudo leer: {e}"))?;
    let export: serde_json::Value =
        serde_json::from_str(&content).map_err(|e| format!("JSON inválido: {e}"))?;

    if export.get("app").and_then(|v| v.as_str()) != Some("codexctl") {
        return Err("No es un archivo de export de codexctl.".into());
    }

    let items = export
        .get("accounts")
        .and_then(|v| v.as_array())
        .ok_or("El archivo no tiene cuentas.")?;

    let mut current = codexctl::manager::load().map_err(|e| e.to_string())?;
    let homes = codexctl::manager::homes_dir();
    let mut imported = 0usize;

    for item in items {
        let uuid = item.get("uuid").and_then(|v| v.as_str()).unwrap_or("");
        let auth = item.get("auth");
        if uuid.is_empty() || auth.is_none() || auth.unwrap().is_null() {
            continue;
        }
        if current.iter().any(|a| a.uuid == uuid) {
            continue; // ya importada
        }

        let home_dir = homes.join(uuid);
        std::fs::create_dir_all(&home_dir).map_err(|e| e.to_string())?;

        let auth_json = serde_json::to_string_pretty(auth.unwrap()).map_err(|e| e.to_string())?;
        std::fs::write(home_dir.join("auth.json"), auth_json + "\n").map_err(|e| e.to_string())?;

        let now = chrono::Utc::now().to_rfc3339();
        let acct = codexctl::models::Account {
            id: item
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("imported")
                .to_string(),
            nickname: item
                .get("nickname")
                .and_then(|v| v.as_str())
                .unwrap_or("Importada")
                .to_string(),
            uuid: uuid.to_string(),
            email: item.get("email").and_then(|v| v.as_str()).map(String::from),
            plan_type: item
                .get("plan_type")
                .and_then(|v| v.as_str())
                .map(String::from),
            provider_account_id: item
                .get("provider_account_id")
                .and_then(|v| v.as_str())
                .map(String::from),
            user_id: item
                .get("user_id")
                .and_then(|v| v.as_str())
                .map(String::from),
            created_at: item
                .get("created_at")
                .and_then(|v| v.as_str())
                .unwrap_or(&now)
                .to_string(),
            updated_at: now.clone(),
        };

        let meta = serde_json::json!({
            "id": acct.id,
            "nickname": acct.nickname,
            "email": acct.email,
            "plan_type": acct.plan_type,
            "created_at": acct.created_at,
            "updated_at": acct.updated_at,
        });
        std::fs::write(
            home_dir.join("meta.json"),
            serde_json::to_string_pretty(&meta).map_err(|e| e.to_string())? + "\n",
        )
        .map_err(|e| e.to_string())?;

        current.push(acct);
        imported += 1;
    }

    codexctl::manager::save(&current).map_err(|e| e.to_string())?;
    Ok(format!("✅ Importadas {imported} cuentas"))
}

async fn quota_for_account(
    client: &reqwest::Client,
    auth_path: &std::path::Path,
) -> Result<QuotaValues, String> {
    if !auth_path.exists() {
        return Err("auth expired".into());
    }
    let creds = codexctl::auth::load(auth_path).map_err(|_| "auth expired".to_string())?;
    let creds = match codexctl::api::maybe_refresh(client, &creds).await {
        Ok(Some(c)) => {
            codexctl::auth::save(&c, auth_path).ok();
            c
        }
        Ok(None) => creds,
        Err(_) => creds,
    };
    let snap = match codexctl::api::fetch_quota(client, &creds).await {
        Ok(snap) => snap,
        Err(normal_error) => {
            let refreshed = match codexctl::api::force_refresh(client, &creds).await {
                Ok(c) => c,
                Err(refresh_error) => {
                    return Err(sanitize_quota_error(
                        &normal_error.to_string(),
                        &refresh_error.to_string(),
                    ));
                }
            };
            codexctl::auth::save(&refreshed, auth_path).ok();
            match codexctl::api::fetch_quota(client, &refreshed).await {
                Ok(snap) => snap,
                Err(retry_error) => {
                    return Err(sanitize_quota_error(
                        &normal_error.to_string(),
                        &retry_error.to_string(),
                    ));
                }
            }
        }
    };
    let p5 = snap
        .primary_window
        .as_ref()
        .map(|w| format!("{:.0}%", w.used_percent))
        .unwrap_or_else(|| "—".into());
    let d5 = snap
        .primary_window
        .as_ref()
        .map(|w| format!("{:.0}%", (100.0 - w.used_percent).max(0.0)))
        .unwrap_or_else(|| "—".into());
    let p7 = snap
        .secondary_window
        .as_ref()
        .map(|w| format!("{:.0}%", w.used_percent))
        .unwrap_or_else(|| "—".into());
    let d7 = snap
        .secondary_window
        .as_ref()
        .map(|w| format!("{:.0}%", (100.0 - w.used_percent).max(0.0)))
        .unwrap_or_else(|| "—".into());
    let reset = snap
        .secondary_window
        .as_ref()
        .or(snap.primary_window.as_ref())
        .and_then(|w| w.reset_at)
        .map(|dt| {
            dt.with_timezone(&chrono::Local)
                .format("%a %d %b %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| "—".into());
    let free_reset_credits = snap
        .rate_limit_reset_credits
        .map(|credits| FreeResetCreditsInfo {
            available_count: credits.available_count,
            applicable_available_count: credits.applicable_available_count,
        });
    let additional = snap
        .additional_rate_limits
        .into_iter()
        .map(|limit| AdditionalRateLimitInfo {
            limit_name: limit.limit_name,
            metered_feature: limit.metered_feature,
            used_percent: limit
                .window
                .as_ref()
                .map(|w| format!("{:.0}%", w.used_percent))
                .unwrap_or_else(|| "—".into()),
            reset_after_seconds: limit.window.as_ref().and_then(|w| w.reset_after_seconds),
            reset_at: limit
                .window
                .and_then(|w| w.reset_at)
                .map(|dt| {
                    dt.with_timezone(&chrono::Local)
                        .format("%a %d %b %H:%M")
                        .to_string()
                })
                .unwrap_or_else(|| "—".into()),
        })
        .collect();
    Ok((p5, d5, p7, d7, reset, free_reset_credits, additional))
}

fn sanitize_quota_error(normal_error: &str, fallback_error: &str) -> String {
    let message = format!("{normal_error} {fallback_error}").to_ascii_lowercase();
    if message.contains("timeout") || message.contains("timed out") {
        "network timeout".into()
    } else if message.contains("401")
        || message.contains("403")
        || message.contains("unauthorized")
        || message.contains("expired")
        || message.contains("revoked")
        || message.contains("reus")
    {
        if message.contains("expired") || message.contains("revoked") || message.contains("reus") {
            "auth expired".into()
        } else {
            "unauthorized".into()
        }
    } else {
        "API unavailable".into()
    }
}

#[cfg(test)]
mod tests {
    use super::sanitize_quota_error;

    #[test]
    fn quota_errors_are_stable_and_do_not_include_details() {
        assert_eq!(
            sanitize_quota_error("API returned 401/403", "ignored token"),
            "unauthorized"
        );
        assert_eq!(
            sanitize_quota_error("refresh_token expirado", "body"),
            "auth expired"
        );
        assert_eq!(
            sanitize_quota_error("error de red: timeout", "body"),
            "network timeout"
        );
        assert_eq!(
            sanitize_quota_error("API HTTP 500: secret response", "body"),
            "API unavailable"
        );
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            list_accounts,
            switch_account,
            switch_account_targets,
            rename_account,
            remove_account,
            get_status,
            get_version,
            add_account,
            reauth_account,
            export_accounts,
            import_accounts,
        ])
        .run(tauri::generate_context!())
        .expect("error running tauri application");
}
