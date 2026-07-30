use chrono::{DateTime, Utc, Datelike, Timelike};
use clap::{Parser, Subcommand};

use crate::api;
use crate::manager;
use crate::models::{Account, QuotaSnapshot, UsageWindow};

const MONTHS: &[&str] = &[
    "ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sep", "oct", "nov", "dic",
];
const DAYS: &[&str] = &["lun", "mar", "mié", "jue", "vie", "sáb", "dom"];

#[derive(Parser)]
#[command(name = "codexctl", version, about = "Multi-account quota tracker para OpenAI Codex")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Mostrar todas las cuentas con quota
    List {
        #[arg(long, help = "Sin fetch de quota (más rápido)")]
        no_quota: bool,
    },
    /// Agregar nueva cuenta (abre el browser)
    Add {
        nickname: String,
    },
    /// Cambiar la cuenta activa
    Switch {
        account: String,
    },
    /// Eliminar una cuenta
    Remove {
        account: String,
    },
    /// Renombrar una cuenta
    Rename {
        account: String,
        nickname: String,
    },
    /// Forzar refresh de tokens
    Refresh {
        account: Option<String>,
    },
    /// Mostrar la cuenta activa
    Status,
    /// Mostrar quota de una o todas
    Quota {
        account: Option<String>,
    },
}

pub async fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let client = reqwest::Client::new();

    match cli.command {
        Commands::List { no_quota } => cmd_list(&client, no_quota).await,
        Commands::Add { nickname } => cmd_add(&nickname),
        Commands::Switch { account } => cmd_switch(&account),
        Commands::Remove { account } => cmd_remove(&account),
        Commands::Rename { account, nickname } => cmd_rename(&account, &nickname),
        Commands::Refresh { account } => cmd_refresh(&client, account.as_deref()).await,
        Commands::Status => cmd_status(),
        Commands::Quota { account } => cmd_quota(&client, account.as_deref()).await,
    }
}

// ── Commands ─────────────────────────────────────────────────────────

async fn cmd_list(client: &reqwest::Client, no_quota: bool) -> anyhow::Result<()> {
    let accounts = manager::load()?;
    if accounts.is_empty() {
        println!("No hay cuentas. Usá 'codexctl add <nickname>' para agregar una.");
        return Ok(());
    }
    let act = manager::active(&accounts);
    let quotas = if no_quota {
        None
    } else {
        eprintln!("→ Consultando quota...");
        Some(fetch_all(client, &accounts).await)
    };
    print_accounts(&accounts, quotas.as_ref(), act.as_ref());
    Ok(())
}

fn cmd_add(nickname: &str) -> anyhow::Result<()> {
    let acct = manager::add(nickname)?;
    println!("✅ Agregada: \"{}\" — ID: {}", acct.nickname, acct.id);
    if let Some(email) = &acct.email {
        println!("   Email: {email}");
    }
    if let Some(plan) = &acct.plan_type {
        println!("   Plan:  {plan}");
    }
    Ok(())
}

fn cmd_switch(account_id: &str) -> anyhow::Result<()> {
    let acct = manager::switch(account_id)?;
    println!("✅ Activada: \"{}\" (ID: {})", acct.nickname, acct.id);
    Ok(())
}

fn cmd_remove(account_id: &str) -> anyhow::Result<()> {
    let accounts = manager::load()?;
    let nick = accounts.iter().find(|a| a.id == account_id || a.nickname == account_id).map(|a| a.nickname.as_str()).unwrap_or(account_id);
    manager::remove(account_id)?;
    println!("✅ Eliminada: \"{nick}\"");
    Ok(())
}

fn cmd_rename(account_id: &str, new_nickname: &str) -> anyhow::Result<()> {
    manager::rename(account_id, new_nickname)?;
    println!("✅ Renombrada a \"{new_nickname}\"");
    Ok(())
}

async fn cmd_refresh(client: &reqwest::Client, account_id: Option<&str>) -> anyhow::Result<()> {
    eprintln!("→ Refrescando tokens y quota...");
    let accounts = manager::load()?;
    let act = manager::active(&accounts);

    let targets: Vec<&Account> = if let Some(id) = account_id {
        vec![manager::resolve(&accounts, id)?]
    } else {
        accounts.iter().collect()
    };

    let quotas = fetch_all_selected(client, &targets).await;
    print_accounts(&accounts, Some(&quotas), act.as_ref());
    Ok(())
}

fn cmd_status() -> anyhow::Result<()> {
    let accounts = manager::load()?;
    let act = manager::active(&accounts);
    match act {
        Some(acct) => {
            println!("✅ Cuenta activa: \"{}\" (ID: {})", acct.nickname, acct.id);
            if let Some(email) = &acct.email {
                println!("   Email: {email}");
            }
            if let Some(plan) = &acct.plan_type {
                println!("   Plan:  {plan}");
            }
        }
        None => {
            println!("❌ No hay ninguna cuenta gestionada activa en ~/.codex/.");
            println!("   Usá 'codexctl switch <id>' para activar una.");
        }
    }
    Ok(())
}

async fn cmd_quota(client: &reqwest::Client, account_id: Option<&str>) -> anyhow::Result<()> {
    let accounts = manager::load()?;
    if accounts.is_empty() {
        println!("No hay cuentas.");
        return Ok(());
    }
    eprintln!("→ Consultando quota...");
    let act = manager::active(&accounts);

    let quotas = if let Some(id) = account_id {
        let target = manager::resolve(&accounts, id)?;
        let mut map = std::collections::HashMap::new();
        map.insert(target.id.clone(), fetch_one(client, target).await);
        map
    } else {
        fetch_all(client, &accounts).await
    };

    print_accounts(&accounts, Some(&quotas), act.as_ref());
    Ok(())
}

// ── Fetch ───────────────────────────────────────────────────────────

async fn fetch_all(client: &reqwest::Client, accounts: &[Account]) -> std::collections::HashMap<String, QuotaSnapshot> {
    let mut map = std::collections::HashMap::new();
    for acct in accounts {
        map.insert(acct.id.clone(), fetch_one(client, acct).await);
    }
    map
}

async fn fetch_all_selected(client: &reqwest::Client, targets: &[&Account]) -> std::collections::HashMap<String, QuotaSnapshot> {
    let mut map = std::collections::HashMap::new();
    for acct in targets {
        map.insert(acct.id.clone(), fetch_one(client, acct).await);
    }
    map
}

async fn fetch_one(client: &reqwest::Client, acct: &Account) -> QuotaSnapshot {
    let auth_path = manager::homes_dir().join(&acct.uuid).join("auth.json");
    if !auth_path.exists() {
        return error_snapshot("no auth.json");
    }

    let creds = match crate::auth::load(&auth_path) {
        Ok(c) => c,
        Err(e) => return error_snapshot(&e.to_string()),
    };

    let creds = match api::maybe_refresh(client, &creds).await {
        Ok(Some(refreshed)) => {
            crate::auth::save(&refreshed, &auth_path).ok();
            refreshed
        }
        _ => creds,
    };

    match api::fetch_quota(client, &creds).await {
        Ok(snap) => snap,
        Err(_) => {
            // Retry with force refresh
            match api::force_refresh(client, &creds).await {
                Ok(refreshed) => {
                    crate::auth::save(&refreshed, &auth_path).ok();
                    api::fetch_quota(client, &refreshed).await.unwrap_or_else(|e| error_snapshot(&e.to_string()))
                }
                Err(e) => error_snapshot(&e.to_string()),
            }
        }
    }
}

fn error_snapshot(msg: &str) -> QuotaSnapshot {
    QuotaSnapshot {
        email: None,
        plan_type: None,
        allowed: None,
        limit_reached: None,
        primary_window: None,
        secondary_window: None,
        credits_balance: None,
        credits_unlimited: None,
    }
}

// ── Display ─────────────────────────────────────────────────────────

fn print_accounts(
    accounts: &[Account],
    quotas: Option<&std::collections::HashMap<String, QuotaSnapshot>>,
    active: Option<&Account>,
) {
    if accounts.is_empty() {
        return;
    }

    let active_id = active.map(|a| a.id.as_str());

    let hdr = row(&["ID", "Cuenta", "Email", "Plan", "5h%", "Disp.5h", "7d%", "Disp.7d", "Reinicio", "Activa"]);
    let sep = row(&["-", "-", "-", "-", "-", "-", "-", "-", "-", "-"]);
    println!("{hdr}");
    println!("{sep}");

    for acct in accounts {
        let snap = quotas.and_then(|q| q.get(&acct.id));
        let line = account_row(acct, snap, active_id == Some(acct.id.as_str()));
        println!("{line}");
    }

    if let Some(quotas) = quotas {
        let ok = quotas.values().filter(|s| s.allowed.unwrap_or(false)).count();
        let total = quotas.len();
        println!();
        println!("📊 {ok}/{total} cuentas con quota activa");
    }
}

fn account_row(acct: &Account, snap: Option<&QuotaSnapshot>, is_active: bool) -> String {
    match snap {
        None => row(&[
            &acct.id, &acct.nickname,
            acct.email.as_deref().unwrap_or("?"),
            acct.plan_type.as_deref().unwrap_or("?"),
            "—", "—", "—", "—", "—",
            if is_active { "✅" } else { "" },
        ]),
        Some(s) => {
            let (p5, d5) = format_quota(&s.primary_window);
            let (p7, d7) = format_quota(&s.secondary_window);
            let reset = s
                .secondary_window
                .as_ref()
                .or(s.primary_window.as_ref())
                .and_then(|w| w.reset_at);
            let reset_str = format_fecha(reset);
            let act = if is_active { "✅" } else { "" };
            row(&[
                &acct.id, &acct.nickname,
                s.email.as_deref().or(acct.email.as_deref()).unwrap_or("?"),
                s.plan_type.as_deref().or(acct.plan_type.as_deref()).unwrap_or("?"),
                &p5, &d5, &p7, &d7, &reset_str, act,
            ])
        }
    }
}

fn row(cols: &[&str]) -> String {
    let widths = [8usize, 20, 34, 8, 8, 8, 8, 8, 20, 7];
    cols.iter()
        .enumerate()
        .map(|(i, c)| {
            let w = widths.get(i).copied().unwrap_or(10);
            if c.len() >= w {
                c.to_string()
            } else {
                format!("{:<width$}", c, width = w)
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn format_quota(w: &Option<UsageWindow>) -> (String, String) {
    match w {
        Some(w) => {
            let used = format!("{:.0}%", w.used_percent);
            let avail = format!("{:.0}%", (100.0 - w.used_percent).max(0.0));
            (used, avail)
        }
        None => ("—".to_string(), "—".to_string()),
    }
}

fn format_fecha(dt: Option<DateTime<Utc>>) -> String {
    match dt {
        None => "—".to_string(),
        Some(dt) => {
            let local = dt.with_timezone(&chrono::Local);
            let now = chrono::Local::now();
            let today = now.date_naive();
            let local_date = local.date_naive();

            if local_date == today {
                format!("hoy {}", local.format("%H:%M"))
            } else if local_date == today.succ() {
                format!("mañana {}", local.format("%H:%M"))
            } else if local.year() == now.year() {
                let dow = DAYS[local.weekday().num_days_from_monday() as usize];
                format!("{dow} {:02} {} {}", local.day(), MONTHS[(local.month0()) as usize], local.format("%H:%M"))
            } else {
                let dow = DAYS[local.weekday().num_days_from_monday() as usize];
                format!("{dow} {:02} {} {} {}", local.day(), MONTHS[(local.month0()) as usize], local.year(), local.format("%H:%M"))
            }
        }
    }
}
