use anyhow::{Context, Result};
use chrono::Utc;
use std::path::PathBuf;
use std::fs;
use uuid::Uuid;

use crate::models::Account;

/// XDG data directory for codexctl.
pub fn data_dir() -> PathBuf {
    let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("codexctl")
}

pub fn homes_dir() -> PathBuf {
    data_dir().join("homes")
}

pub fn accounts_file() -> PathBuf {
    data_dir().join("accounts.json")
}

pub fn ambient_home() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".codex")
}

pub fn ambient_auth() -> PathBuf {
    ambient_home().join("auth.json")
}

/// Load accounts from accounts.json + discover ambient.
pub fn load() -> Result<Vec<Account>> {
    let d = data_dir();
    fs::create_dir_all(&d).ok();
    fs::create_dir_all(homes_dir()).ok();

    let mut accounts: Vec<Account> = Vec::new();

    if accounts_file().exists() {
        let text = fs::read_to_string(accounts_file())?;
        if let Ok(list) = serde_json::from_str::<Vec<Account>>(&text) {
            accounts = list;
        }
    }

    // Drop accounts whose home dir vanished
    accounts.retain(|a| a.uuid == "__ambient__" || homes_dir().join(&a.uuid).exists());

    Ok(accounts)
}

/// Save accounts to accounts.json.
pub fn save(accounts: &[Account]) -> Result<()> {
    let d = data_dir();
    fs::create_dir_all(&d)?;
    let text = serde_json::to_string_pretty(accounts)?;
    fs::write(accounts_file(), text + "\n")?;
    Ok(())
}

/// Add a new account by running `codex login` in an isolated home.
pub fn add(nickname: &str) -> Result<Account> {
    let home_uuid = Uuid::new_v4().to_string();
    let home_dir = homes_dir().join(&home_uuid);
    fs::create_dir_all(&home_dir)?;

    let binary = which_codex().context("codex binary no encontrado en PATH")?;

    let mut cmd = std::process::Command::new(&binary);
    cmd.arg("login");
    cmd.env("CODEX_HOME", &home_dir);

    eprintln!("→ Browser abierto para \"{}\".", nickname);

    let status = cmd.status().context("fallo al ejecutar codex login")?;
    if !status.success() {
        fs::remove_dir_all(&home_dir).ok();
        anyhow::bail!("codex login falló (exit {})", status);
    }

    let auth_path = home_dir.join("auth.json");
    if !auth_path.exists() {
        fs::remove_dir_all(&home_dir).ok();
        anyhow::bail!("codex login completado pero no se creó auth.json");
    }

    let mut creds = crate::auth::load(&auth_path)?;
    let now = Utc::now().to_rfc3339();
    let plan = creds.plan_type.clone().unwrap_or_else(|| "unknown".into());
    let sid = format!("{}-{}", plan, count_by_plan_type(plan.as_str()) + 1);

    let acct = Account {
        id: sid,
        nickname: nickname.to_string(),
        uuid: home_uuid,
        email: creds.email.take(),
        plan_type: Some(plan),
        provider_account_id: creds.account_id.take(),
        created_at: now.clone(),
        updated_at: now,
    };

    write_meta(&acct)?;

    let mut all = load()?;
    all.push(acct.clone());
    save(&all)?;

    Ok(acct)
}

/// Remove a managed account.
pub fn remove(account_id: &str) -> Result<()> {
    let mut all = load()?;
    let idx = find_index(&all, account_id)
        .ok_or_else(|| anyhow::anyhow!("Cuenta '{}' no encontrada", account_id))?;
    let acct = &all[idx];
    let home = homes_dir().join(&acct.uuid);
    if acct.uuid != "__ambient__" && home.exists() {
        fs::remove_dir_all(&home).ok();
    }
    all.remove(idx);
    save(&all)
}

/// Rename an account.
pub fn rename(account_id: &str, new_nickname: &str) -> Result<Account> {
    let mut all = load()?;
    let idx = find_index(&all, account_id)
        .ok_or_else(|| anyhow::anyhow!("Cuenta '{}' no encontrada", account_id))?;
    all[idx].nickname = new_nickname.to_string();
    all[idx].updated_at = Utc::now().to_rfc3339();
    write_meta(&all[idx])?;
    save(&all)?;
    Ok(all[idx].clone())
}

/// Switch active account by copying auth.json to ~/.codex/.
pub fn switch(account_id: &str) -> Result<Account> {
    let all = load()?;
    let acct = resolve(&all, account_id)?;
    let src = homes_dir().join(&acct.uuid).join("auth.json");
    if !src.exists() {
        anyhow::bail!("La cuenta '{}' no tiene auth.json", acct.id);
    }

    let dest = ambient_auth();
    fs::create_dir_all(ambient_home())?;
    fs::copy(&src, &dest)?;

    // chmod 600 (solo Unix)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&dest, fs::Permissions::from_mode(0o600))?;
    }

    Ok(acct.clone())
}

// ── OpenCode integration ────────────────────────────────────────────

/// Path to OpenCode's auth store.
pub fn opencode_auth_path() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("opencode")
        .join("auth.json")
}

/// Switch the OpenAI account used by OpenCode (writes its auth store).
///
/// OpenCode stores `{type: "oauth", access, refresh, expires, accountId}`
/// under the `openai` key. We refresh the account's tokens if stale,
/// then copy them there, deriving `expires` from the JWT `exp` claim.
pub async fn opencode_switch(client: &reqwest::Client, account_id: &str) -> Result<Account> {
    let all = load()?;
    let acct = resolve(&all, account_id)?;
    let src = homes_dir().join(&acct.uuid).join("auth.json");
    if !src.exists() {
        anyhow::bail!("La cuenta '{}' no tiene auth.json", acct.id);
    }
    let mut creds = crate::auth::load(&src)?;

    // Refresh tokens first if stale, so OpenCode gets fresh ones
    match crate::api::maybe_refresh(client, &creds).await {
        Ok(Some(fresh)) => {
            crate::auth::save(&fresh, &src)?;
            creds = fresh;
        }
        Ok(None) => {}
        Err(e) => {
            eprintln!("⚠️  no se pudo refrescar tokens (se usan los actuales): {e}");
        }
    }

    let path = opencode_auth_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    // Backup before touching
    if path.exists() {
        let bak = path.with_extension("json.bak");
        fs::copy(&path, &bak)?;
    }

    let mut store: serde_json::Value = if path.exists() {
        serde_json::from_str(&fs::read_to_string(&path)?).unwrap_or(serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    let expires_ms = jwt_exp_ms(&creds.access_token).unwrap_or(0);
    store["openai"] = serde_json::json!({
        "type": "oauth",
        "access": creds.access_token,
        "refresh": creds.refresh_token,
        "expires": expires_ms,
        "accountId": creds.account_id.clone().unwrap_or_default(),
    });

    fs::write(&path, serde_json::to_string_pretty(&store)? + "\n")?;
    Ok(acct.clone())
}

/// Info about the account OpenCode currently uses.
pub struct OpencodeInfo {
    pub account_id: String,
    pub expires_at: Option<i64>, // epoch ms
    pub has_refresh: bool,
    pub matched_account: Option<Account>,
}

/// Read OpenCode's auth store and report which OpenAI account it uses.
pub fn opencode_status() -> Result<OpencodeInfo> {
    let path = opencode_auth_path();
    if !path.exists() {
        anyhow::bail!("No existe el auth de OpenCode en {}", path.display());
    }
    let store: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path)?)?;
    let oai = store
        .get("openai")
        .ok_or_else(|| anyhow::anyhow!("OpenCode no tiene provider 'openai' configurado."))?;

    let account_id = oai
        .get("accountId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let expires_at = oai.get("expires").and_then(|v| v.as_i64());
    let has_refresh = oai
        .get("refresh")
        .and_then(|v| v.as_str())
        .map(|s| !s.is_empty())
        .unwrap_or(false);

    let all = load()?;
    let matched = all
        .iter()
        .find(|a| a.provider_account_id.as_deref() == Some(account_id.as_str()))
        .cloned();

    Ok(OpencodeInfo {
        account_id,
        expires_at,
        has_refresh,
        matched_account: matched,
    })
}

/// Decode the `exp` claim (seconds) of a JWT and return milliseconds.
fn jwt_exp_ms(token: &str) -> Option<i64> {
    let part = token.split('.').nth(1)?;
    let padded = format!("{}{}", part, "=".repeat((4 - part.len() % 4) % 4));
    use base64::Engine;
    let bytes = base64::engine::general_purpose::URL_SAFE.decode(padded.as_bytes()).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    value.get("exp")?.as_i64().map(|s| s * 1000)
}

/// Find which account is currently active (auth matches ~/.codex/auth.json).
pub fn active(accounts: &[Account]) -> Option<Account> {
    if !ambient_auth().exists() {
        return None;
    }
    let ambient_bytes = fs::read(ambient_auth()).ok()?;

    for acct in accounts {
        let acct_auth = homes_dir().join(&acct.uuid).join("auth.json");
        if !acct_auth.exists() {
            continue;
        }
        if let Ok(bytes) = fs::read(&acct_auth) {
            if bytes == ambient_bytes {
                return Some(acct.clone());
            }
        }
    }
    None
}

// ── Helpers ─────────────────────────────────────────────────────────

fn which_codex() -> Option<PathBuf> {
    // On Windows the binary is codex.exe; on Unix it's codex
    let exe = if cfg!(windows) { "codex.exe" } else { "codex" };
    std::env::var_os("PATH")
        .as_ref()
        .and_then(|path| {
            std::env::split_paths(path).find_map(|dir| {
                let candidate = dir.join(exe);
                if candidate.is_file() {
                    Some(candidate)
                } else {
                    None
                }
            })
        })
}

fn count_by_plan_type(plan: &str) -> usize {
    let all = load().unwrap_or_default();
    all.iter()
        .filter(|a| a.plan_type.as_deref() == Some(plan))
        .count()
}

fn write_meta(acct: &Account) -> Result<()> {
    if acct.uuid == "__ambient__" {
        return Ok(());
    }
    let home = homes_dir().join(&acct.uuid);
    fs::create_dir_all(&home)?;
    let meta = serde_json::json!({
        "id": acct.id,
        "nickname": acct.nickname,
        "email": acct.email,
        "plan_type": acct.plan_type,
        "created_at": acct.created_at,
        "updated_at": acct.updated_at,
    });
    fs::write(home.join("meta.json"), serde_json::to_string_pretty(&meta)? + "\n")?;
    Ok(())
}

pub fn resolve<'a>(accounts: &'a [Account], key: &str) -> Result<&'a Account> {
    find_index(accounts, key)
        .map(|i| &accounts[i])
        .ok_or_else(|| anyhow::anyhow!("Cuenta '{}' no encontrada. Usá 'list' para ver disponibles.", key))
}

fn find_index(accounts: &[Account], key: &str) -> Option<usize> {
    let key_lower = key.to_lowercase().trim().to_string();
    // Exact match
    for (i, a) in accounts.iter().enumerate() {
        if a.id == key || a.nickname == key {
            return Some(i);
        }
    }
    // Prefix match
    let matches: Vec<usize> = accounts
        .iter()
        .enumerate()
        .filter(|(_, a)| {
            a.id.to_lowercase().starts_with(&key_lower)
                || a.nickname.to_lowercase().starts_with(&key_lower)
        })
        .map(|(i, _)| i)
        .collect();
    if matches.len() == 1 {
        return Some(matches[0]);
    }
    None
}
