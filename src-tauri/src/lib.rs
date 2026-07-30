use serde::Serialize;

#[derive(Serialize)]
struct AccountInfo {
    id: String,
    nickname: String,
    email: String,
    plan_type: String,
    is_active: bool,
    quota_5h: String,
    disp_5h: String,
    quota_7d: String,
    disp_7d: String,
    reset_at: String,
}

#[tauri::command]
async fn list_accounts(fetch_quota: bool) -> Result<Vec<AccountInfo>, String> {
    let client = reqwest::Client::new();
    let accounts = codexctl::manager::load().map_err(|e| e.to_string())?;
    let active = codexctl::manager::active(&accounts);
    let mut result = Vec::new();
    for acct in &accounts {
        let is_active = active.as_ref().map(|a| a.id == acct.id).unwrap_or(false);
        let (p5, d5, p7, d7, reset) = if fetch_quota {
            let auth_path = codexctl::manager::homes_dir().join(&acct.uuid).join("auth.json");
            if let Some(q) = quota_for_account(&client, &auth_path).await {
                q
            } else {
                ("—".into(), "—".into(), "—".into(), "—".into(), "—".into())
            }
        } else {
            ("—".into(), "—".into(), "—".into(), "—".into(), "—".into())
        };
        result.push(AccountInfo {
            id: acct.id.clone(),
            nickname: acct.nickname.clone(),
            email: acct.email.clone().unwrap_or_default(),
            plan_type: acct.plan_type.clone().unwrap_or_default(),
            is_active,
            quota_5h: p5,
            disp_5h: d5,
            quota_7d: p7,
            disp_7d: d7,
            reset_at: reset,
        });
    }
    Ok(result)
}

#[tauri::command]
async fn switch_account(account_id: String) -> Result<String, String> {
    let acct = codexctl::manager::switch(&account_id).map_err(|e| e.to_string())?;
    Ok(format!("✅ Activada: {}", acct.nickname))
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
async fn add_account(nickname: String) -> Result<String, String> {
    let acct = codexctl::manager::add(&nickname).map_err(|e| e.to_string())?;
    Ok(format!("✅ Agregada: \"{}\" (ID: {})", acct.nickname, acct.id))
}

async fn quota_for_account(client: &reqwest::Client, auth_path: &std::path::Path) -> Option<(String, String, String, String, String)> {
    if !auth_path.exists() { return None; }
    let creds = codexctl::auth::load(auth_path).ok()?;
    let creds = match codexctl::api::maybe_refresh(client, &creds).await.ok()? {
        Some(c) => { codexctl::auth::save(&c, auth_path).ok(); c }
        None => creds,
    };
    let snap = codexctl::api::fetch_quota(client, &creds).await.ok()?;
    let p5 = snap.primary_window.as_ref().map(|w| format!("{:.0}%", w.used_percent)).unwrap_or_else(|| "—".into());
    let d5 = snap.primary_window.as_ref().map(|w| format!("{:.0}%", (100.0 - w.used_percent).max(0.0))).unwrap_or_else(|| "—".into());
    let p7 = snap.secondary_window.as_ref().map(|w| format!("{:.0}%", w.used_percent)).unwrap_or_else(|| "—".into());
    let d7 = snap.secondary_window.as_ref().map(|w| format!("{:.0}%", (100.0 - w.used_percent).max(0.0))).unwrap_or_else(|| "—".into());
    let reset = snap.secondary_window.as_ref().or(snap.primary_window.as_ref())
        .and_then(|w| w.reset_at)
        .map(|dt| dt.with_timezone(&chrono::Local).format("%a %d %b %H:%M").to_string())
        .unwrap_or_else(|| "—".into());
    Some((p5, d5, p7, d7, reset))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            list_accounts, switch_account, rename_account,
            remove_account, get_status, add_account,
        ])
        .run(tauri::generate_context!())
        .expect("error running tauri application");
}
