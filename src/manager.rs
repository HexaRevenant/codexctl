use anyhow::{Context, Result};
use chrono::Utc;
use serde_json::Value;
use std::path::{Path, PathBuf};
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
    std::env::var_os("PATH")
        .as_ref()
        .and_then(|path| {
            std::env::split_paths(path).find_map(|dir| {
                let candidate = dir.join("codex");
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
