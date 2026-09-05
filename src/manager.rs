use anyhow::{Context, Result};
use chrono::Utc;
use std::fs;
use std::path::{Path, PathBuf};
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
        user_id: creds.user_id.take(),
        created_at: now.clone(),
        updated_at: now,
    };

    write_meta(&acct)?;

    let mut all = load()?;
    all.push(acct.clone());
    save(&all)?;

    Ok(acct)
}

/// Re-authenticate an existing account without changing its local identity.
pub fn reauth(account_id: &str) -> Result<Account> {
    let mut all = load()?;
    let idx = find_index(&all, account_id)
        .ok_or_else(|| anyhow::anyhow!("Cuenta '{}' no encontrada", account_id))?;
    let original = all[idx].clone();
    let home_dir = homes_dir().join(&original.uuid);
    let auth_path = home_dir.join("auth.json");
    let backup_path = home_dir.join(format!("auth.json.reauth-{}.bak", Uuid::new_v4()));

    if auth_path.exists() {
        fs::rename(&auth_path, &backup_path)?;
    }

    let result = (|| -> Result<Account> {
        let binary = which_codex().context("codex binary no encontrado en PATH")?;
        let status = std::process::Command::new(binary)
            .arg("login")
            .env("CODEX_HOME", &home_dir)
            .status()
            .context("fallo al ejecutar codex login")?;
        if !status.success() {
            anyhow::bail!("codex login falló (exit {})", status.code().unwrap_or(-1));
        }
        if !auth_path.exists() {
            anyhow::bail!("codex login completado pero no se creó auth.json");
        }

        let creds =
            crate::auth::load(&auth_path).context("codex login produjo un auth.json inválido")?;
        let updated = account_with_reauth(&original, creds);

        all[idx] = updated.clone();
        write_meta(&updated)?;
        save(&all)?;
        Ok(updated)
    })();

    match result {
        Ok(updated) => {
            fs::remove_file(&backup_path).ok();
            Ok(updated)
        }
        Err(error) => {
            restore_auth(&auth_path, &backup_path);
            Err(error)
        }
    }
}

fn account_with_reauth(original: &Account, creds: crate::models::AuthCredentials) -> Account {
    let mut updated = original.clone();
    updated.email = creds.email;
    updated.plan_type = creds.plan_type;
    updated.provider_account_id = creds.account_id;
    updated.user_id = creds.user_id;
    updated.updated_at = Utc::now().to_rfc3339();
    updated
}

fn restore_auth(auth_path: &std::path::Path, backup_path: &std::path::Path) {
    fs::remove_file(auth_path).ok();
    if backup_path.exists() {
        fs::rename(backup_path, auth_path).ok();
    }
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
    let bytes = base64::engine::general_purpose::URL_SAFE
        .decode(padded.as_bytes())
        .ok()?;
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

/// Where each account is active: "codex", "opencode", "both", or "".
pub fn active_targets(accounts: &[Account]) -> std::collections::HashMap<String, String> {
    let mut map: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for a in accounts {
        map.insert(a.id.clone(), String::new());
    }

    // Codex: auth.json bytes match ~/.codex/auth.json
    if let Ok(ambient_bytes) = fs::read(ambient_auth()) {
        for acct in accounts {
            let acct_auth = homes_dir().join(&acct.uuid).join("auth.json");
            if let Ok(bytes) = fs::read(&acct_auth) {
                if bytes == ambient_bytes {
                    map.entry(acct.id.clone()).and_modify(|v| {
                        *v = if v.is_empty() {
                            "codex".into()
                        } else {
                            "both".into()
                        }
                    });
                }
            }
        }
    }

    // OpenCode: first try EXACT token match (the home whose access token
    // equals the one OpenCode holds). Falls back to user_id, then accountId.
    let oc_access = opencode_access_token();
    let oc_user_id = opencode_access_user_id();
    let oc_id = opencode_status().map(|i| i.account_id).unwrap_or_default();

    for acct in accounts {
        let acct_uid = acct.user_id.clone().or_else(|| {
            let p = homes_dir().join(&acct.uuid).join("auth.json");
            crate::auth::load(&p).ok().and_then(|c| c.user_id)
        });

        let matches = if let Some(oc_tok) = &oc_access {
            // Exact: same access token → same home
            let acct_auth_path = homes_dir().join(&acct.uuid).join("auth.json");
            let acct_tok = crate::auth::load(&acct_auth_path)
                .ok()
                .map(|c| c.access_token);
            acct_tok.as_deref() == Some(oc_tok.as_str())
        } else if let Some(uid) = &oc_user_id {
            acct_uid.as_deref() == Some(uid.as_str())
        } else {
            !oc_id.is_empty() && acct.provider_account_id.as_deref() == Some(oc_id.as_str())
        };

        if matches {
            map.entry(acct.id.clone()).and_modify(|v| {
                *v = if v.is_empty() {
                    "opencode".into()
                } else {
                    "both".into()
                }
            });
        }
    }

    map
}

/// The raw access token that OpenCode currently holds for `openai`.
fn opencode_access_token() -> Option<String> {
    let path = opencode_auth_path();
    if !path.exists() {
        return None;
    }
    let store: serde_json::Value = serde_json::from_str(&fs::read_to_string(path).ok()?).ok()?;
    store
        .get("openai")?
        .get("access")?
        .as_str()
        .map(String::from)
}

/// Decode the user_id (chatgpt_user_id) from the access token that
/// OpenCode currently holds for its `openai` provider.
fn opencode_access_user_id() -> Option<String> {
    let path = opencode_auth_path();
    if !path.exists() {
        return None;
    }
    let store: serde_json::Value = serde_json::from_str(&fs::read_to_string(path).ok()?).ok()?;
    let access = store.get("openai")?.get("access")?.as_str()?;
    let part = access.split('.').nth(1)?;
    let padded = format!("{}{}", part, "=".repeat((4 - part.len() % 4) % 4));
    use base64::Engine;
    let bytes = base64::engine::general_purpose::URL_SAFE
        .decode(padded.as_bytes())
        .ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let auth = value.get("https://api.openai.com/auth")?;
    auth.get("chatgpt_user_id")?.as_str().map(String::from)
}

// ── Helpers ─────────────────────────────────────────────────────────

fn which_codex() -> Option<PathBuf> {
    // On Windows the binary is codex.exe; on Unix it's codex
    let exe = if cfg!(windows) { "codex.exe" } else { "codex" };
    find_codex(std::env::var_os("PATH"), dirs::home_dir(), exe)
}

fn find_codex(
    path: Option<std::ffi::OsString>,
    home: Option<PathBuf>,
    exe: &str,
) -> Option<PathBuf> {
    let mut candidates = path
        .into_iter()
        .flat_map(|value| {
            std::env::split_paths(&value)
                .map(|dir| dir.join(exe))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();

    if let Some(home) = home {
        candidates.extend([
            home.join(".local/bin").join(exe),
            home.join(".cargo/bin").join(exe),
            home.join(".npm-global/bin").join(exe),
            home.join(".bun/bin").join(exe),
            home.join(".linuxbrew/bin").join(exe),
        ]);
    }
    candidates.extend([
        PathBuf::from("/home/linuxbrew/.linuxbrew/bin").join(exe),
        PathBuf::from("/usr/local/bin").join(exe),
        PathBuf::from("/usr/bin").join(exe),
    ]);

    candidates
        .into_iter()
        .find(|candidate| is_executable_file(candidate))
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
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
        "provider_account_id": acct.provider_account_id,
        "user_id": acct.user_id,
        "created_at": acct.created_at,
        "updated_at": acct.updated_at,
    });
    fs::write(
        home.join("meta.json"),
        serde_json::to_string_pretty(&meta)? + "\n",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reauth_preserves_account_identity_fields() {
        let original = Account {
            id: "pro-1".into(),
            nickname: "Personal".into(),
            uuid: "home-uuid".into(),
            email: Some("old@example.com".into()),
            plan_type: Some("plus".into()),
            provider_account_id: Some("old-provider".into()),
            user_id: Some("old-user".into()),
            created_at: "2024-01-01T00:00:00Z".into(),
            updated_at: "2024-01-02T00:00:00Z".into(),
        };
        let updated = account_with_reauth(
            &original,
            crate::models::AuthCredentials {
                access_token: String::new(),
                refresh_token: String::new(),
                id_token: None,
                account_id: Some("new-provider".into()),
                last_refresh: None,
                email: Some("new@example.com".into()),
                name: None,
                plan_type: Some("team".into()),
                user_id: Some("new-user".into()),
            },
        );

        assert_eq!(updated.id, original.id);
        assert_eq!(updated.nickname, original.nickname);
        assert_eq!(updated.uuid, original.uuid);
        assert_eq!(updated.created_at, original.created_at);
    }

    #[test]
    fn failed_reauth_restores_auth_backup() {
        let dir = std::env::temp_dir().join(format!("codexctl-reauth-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let auth = dir.join("auth.json");
        let backup = dir.join("auth.json.reauth-test.bak");
        fs::write(&auth, "old credentials").unwrap();
        fs::rename(&auth, &backup).unwrap();
        fs::write(&auth, "partial login output").unwrap();

        restore_auth(&auth, &backup);

        assert_eq!(fs::read_to_string(&auth).unwrap(), "old credentials");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn codex_discovery_uses_user_fallback_when_path_is_empty() {
        let dir = std::env::temp_dir().join(format!("codexctl-which-{}", Uuid::new_v4()));
        let bin_dir = dir.join(".local/bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let binary = bin_dir.join("codex");
        fs::write(&binary, "#!/bin/sh\n").unwrap();
        make_executable(&binary);

        assert_eq!(
            find_codex(Some("".into()), Some(dir.clone()), "codex"),
            Some(binary)
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn codex_discovery_skips_non_executable_path_entry() {
        let dir = std::env::temp_dir().join(format!("codexctl-which-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let binary = dir.join("codexctl-test-not-executable");
        fs::write(&binary, "not executable").unwrap();

        assert_eq!(
            find_codex(
                Some(dir.into_os_string()),
                None,
                "codexctl-test-not-executable"
            ),
            None
        );
        fs::remove_dir_all(binary.parent().unwrap()).unwrap();
    }

    #[cfg(unix)]
    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(not(unix))]
    fn make_executable(_path: &Path) {}
}

pub fn resolve<'a>(accounts: &'a [Account], key: &str) -> Result<&'a Account> {
    find_index(accounts, key)
        .map(|i| &accounts[i])
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Cuenta '{}' no encontrada. Usá 'list' para ver disponibles.",
                key
            )
        })
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
