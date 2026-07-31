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

/// Switch account to Codex, OpenCode, or both.
/// targets is a subset of ["codex", "opencode"].
#[tauri::command]
async fn switch_account_targets(account_id: String, targets: Vec<String>) -> Result<String, String> {
    let mut done: Vec<String> = Vec::new();

    if targets.iter().any(|t| t == "codex") || targets.is_empty() {
        let acct = codexctl::manager::switch(&account_id).map_err(|e| e.to_string())?;
        done.push(format!("Codex → {}", acct.nickname));
    }
    if targets.iter().any(|t| t == "opencode") {
        let client = reqwest::Client::new();
        let acct = codexctl::manager::opencode_switch(&client, &account_id).await.map_err(|e| e.to_string())?;
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
async fn add_account(nickname: String) -> Result<String, String> {
    let acct = codexctl::manager::add(&nickname).map_err(|e| e.to_string())?;
    Ok(format!("✅ Agregada: \"{}\" (ID: {})", acct.nickname, acct.id))
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

    std::fs::write(&path, serde_json::to_string_pretty(&export).map_err(|e| e.to_string())?)
        .map_err(|e| format!("no se pudo escribir: {e}"))?;

    Ok(format!("✅ Exportadas {} cuentas a {}", accounts.len(), path))
}

/// Import accounts from an exported JSON file (skips already-known uuids).
#[tauri::command]
async fn import_accounts(path: String) -> Result<String, String> {
    let content = std::fs::read_to_string(&path).map_err(|e| format!("no se pudo leer: {e}"))?;
    let export: serde_json::Value = serde_json::from_str(&content).map_err(|e| format!("JSON inválido: {e}"))?;

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
            id: item.get("id").and_then(|v| v.as_str()).unwrap_or("imported").to_string(),
            nickname: item.get("nickname").and_then(|v| v.as_str()).unwrap_or("Importada").to_string(),
            uuid: uuid.to_string(),
            email: item.get("email").and_then(|v| v.as_str()).map(String::from),
            plan_type: item.get("plan_type").and_then(|v| v.as_str()).map(String::from),
            provider_account_id: item.get("provider_account_id").and_then(|v| v.as_str()).map(String::from),
            created_at: item.get("created_at").and_then(|v| v.as_str()).unwrap_or(&now).to_string(),
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
        std::fs::write(home_dir.join("meta.json"), serde_json::to_string_pretty(&meta).map_err(|e| e.to_string())? + "\n")
            .map_err(|e| e.to_string())?;

        current.push(acct);
        imported += 1;
    }

    codexctl::manager::save(&current).map_err(|e| e.to_string())?;
    Ok(format!("✅ Importadas {imported} cuentas"))
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
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            list_accounts, switch_account, switch_account_targets, rename_account,
            remove_account, get_status, add_account,
            export_accounts, import_accounts,
        ])
        .run(tauri::generate_context!())
        .expect("error running tauri application");
}
