use std::{
    collections::HashMap,
    io::{self, stdout},
    time::{Duration, Instant},
};

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState},
    Terminal,
};

use crate::{
    api, auth, manager,
    models::{Account, QuotaSnapshot, UsageWindow},
};

const POLL: Duration = Duration::from_millis(100);
const AUTO_REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);
const REQUEST_LIMIT: Duration = Duration::from_secs(35);

#[derive(Clone, Debug, Default)]
struct AccountView {
    quota: Option<QuotaSnapshot>,
    error: Option<String>,
}

#[derive(Clone, Copy, Debug)]
enum RefreshSource {
    Manual,
    Automatic,
}

#[derive(Debug)]
enum WorkerMessage {
    Quotas {
        views: HashMap<String, AccountView>,
        source: RefreshSource,
    },
    Switched(String),
    Reauthenticated(String),
    Error(String),
}

struct Model {
    accounts: Vec<Account>,
    views: HashMap<String, AccountView>,
    selected: usize,
    table: TableState,
    loading: bool,
    status: String,
    last_refresh: Option<Instant>,
    next_auto_refresh: Instant,
    horizontal_offset: usize,
    tx: tokio::sync::mpsc::UnboundedSender<WorkerMessage>,
    rx: tokio::sync::mpsc::UnboundedReceiver<WorkerMessage>,
}

impl Model {
    fn new(accounts: Vec<Account>) -> Self {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let mut table = TableState::default();
        table.select((!accounts.is_empty()).then_some(0));
        Self {
            accounts,
            views: HashMap::new(),
            selected: 0,
            table,
            loading: false,
            status: "Listo — pulsa r para actualizar las cuotas".into(),
            last_refresh: None,
            next_auto_refresh: Instant::now() + AUTO_REFRESH_INTERVAL,
            horizontal_offset: 0,
            tx,
            rx,
        }
    }

    fn selected_account(&self) -> Option<&Account> {
        self.accounts.get(self.selected)
    }

    fn start_refresh(&mut self, source: RefreshSource) {
        if self.loading || self.accounts.is_empty() {
            return;
        }
        self.loading = true;
        self.status = match source {
            RefreshSource::Manual => "Actualizando cuotas…".into(),
            RefreshSource::Automatic => "Actualización automática iniciada…".into(),
        };
        self.last_refresh = Some(Instant::now());
        self.next_auto_refresh = Instant::now() + AUTO_REFRESH_INTERVAL;
        let accounts = self.accounts.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let _ = tx.send(WorkerMessage::Quotas {
                views: refresh(accounts).await,
                source,
            });
        });
    }

    fn maybe_start_auto_refresh(&mut self, now: Instant) {
        if auto_refresh_due(now, self.next_auto_refresh, self.loading) {
            self.start_refresh(RefreshSource::Automatic);
        }
    }

    fn handle_message(&mut self, message: WorkerMessage) {
        self.loading = false;
        match message {
            WorkerMessage::Quotas { views, source } => {
                let failures = views.values().filter(|v| v.error.is_some()).count();
                self.views = views;
                self.status = if failures == 0 {
                    match source {
                        RefreshSource::Manual => "Cuotas actualizadas".into(),
                        RefreshSource::Automatic => "Actualización automática completada".into(),
                    }
                } else {
                    match source {
                        RefreshSource::Manual => {
                            format!("Actualizado con {failures} error(es) visible(s) de autenticación/API")
                        }
                        RefreshSource::Automatic => {
                            format!(
                                "Actualización automática completada con {failures} error(es) visible(s) de autenticación/API"
                            )
                        }
                    }
                };
            }
            WorkerMessage::Switched(name) => self.status = format!("Cuenta activa: {name}"),
            WorkerMessage::Reauthenticated(name) => {
                self.status = format!("Cuenta reautenticada: {name}");
                self.start_refresh(RefreshSource::Manual);
            }
            WorkerMessage::Error(error) => self.status = error,
        }
    }

    fn move_selection(&mut self, delta: isize) {
        if self.accounts.is_empty() {
            return;
        }
        self.selected =
            (self.selected as isize + delta).rem_euclid(self.accounts.len() as isize) as usize;
        self.table.select(Some(self.selected));
        self.status = format!("Seleccionada: {}", self.accounts[self.selected].nickname);
    }
}

fn auto_refresh_due(now: Instant, next_refresh: Instant, loading: bool) -> bool {
    !loading && now >= next_refresh
}

pub async fn run() -> Result<()> {
    let accounts = manager::load()?;
    let mut terminal = setup_terminal()?;
    let result = run_loop(&mut terminal, accounts).await;
    restore_terminal(&mut terminal)?;
    result
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    // The panic hook is deliberately installed here: terminal state must be restored even
    // when a rendering panic escapes the event loop.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic| {
        let _ = restore_terminal_raw();
        previous(panic);
    }));
    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen, crossterm::cursor::Hide)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(out))?;
    terminal.clear()?;
    Ok(terminal)
}

fn restore_terminal_raw() -> io::Result<()> {
    disable_raw_mode().ok();
    execute!(stdout(), LeaveAlternateScreen, crossterm::cursor::Show)
}

fn restore_terminal<B: ratatui::backend::Backend>(terminal: &mut Terminal<B>) -> Result<()> {
    terminal.show_cursor().ok();
    restore_terminal_raw()?;
    Ok(())
}

async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    accounts: Vec<Account>,
) -> Result<()> {
    let mut model = Model::new(accounts);
    model.start_refresh(RefreshSource::Manual);
    loop {
        terminal.draw(|frame| render(frame, &mut model))?;
        while let Ok(message) = model.rx.try_recv() {
            model.handle_message(message);
        }
        model.maybe_start_auto_refresh(Instant::now());
        if event::poll(POLL)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press && handle_key(&mut model, key).await? {
                    break;
                }
            }
        }
    }
    Ok(())
}

async fn handle_key(model: &mut Model, key: KeyEvent) -> Result<bool> {
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => return Ok(true),
        KeyCode::Up => model.move_selection(-1),
        KeyCode::Down => model.move_selection(1),
        KeyCode::Left => model.horizontal_offset = model.horizontal_offset.saturating_sub(1),
        KeyCode::Right => model.horizontal_offset = (model.horizontal_offset + 1).min(7),
        KeyCode::Char('r') => model.start_refresh(RefreshSource::Manual),
        KeyCode::Enter | KeyCode::Char('s') => switch_selected(model),
        KeyCode::Char('a') => reauth_selected(model),
        _ => {}
    }
    Ok(false)
}

fn switch_selected(model: &mut Model) {
    let Some(account) = model.selected_account().cloned() else {
        return;
    };
    model.loading = true;
    model.status = format!("Activando {}…", account.nickname);
    let tx = model.tx.clone();
    tokio::task::spawn_blocking(move || {
        let message = manager::switch(&account.id)
            .map(|a| WorkerMessage::Switched(a.nickname))
            .unwrap_or_else(|e| WorkerMessage::Error(format!("No se pudo activar la cuenta: {e}")));
        let _ = tx.send(message);
    });
}

fn reauth_selected(model: &mut Model) {
    let Some(account) = model.selected_account().cloned() else {
        return;
    };
    model.loading = true;
    model.status = format!(
        "Reautenticando {} — puede abrirse el navegador…",
        account.nickname
    );
    let tx = model.tx.clone();
    tokio::task::spawn_blocking(move || {
        let message = manager::reauth(&account.id)
            .map(|a| WorkerMessage::Reauthenticated(a.nickname))
            .unwrap_or_else(|e| WorkerMessage::Error(format!("No se pudo reautenticar: {e}")));
        let _ = tx.send(message);
    });
}

async fn refresh(accounts: Vec<Account>) -> HashMap<String, AccountView> {
    let client = reqwest::Client::new();
    let mut views = HashMap::new();
    for account in accounts {
        let result = tokio::time::timeout(REQUEST_LIMIT, fetch_account(&client, &account)).await;
        let view = match result {
            Ok(Ok(quota)) => AccountView {
                quota: Some(quota),
                error: None,
            },
            Ok(Err(error)) => AccountView {
                quota: None,
                error: Some(safe_error(&error.to_string())),
            },
            Err(_) => AccountView {
                quota: None,
                error: Some("La consulta de cuota agotó el tiempo de espera".into()),
            },
        };
        views.insert(account.id, view);
    }
    views
}

async fn fetch_account(client: &reqwest::Client, account: &Account) -> Result<QuotaSnapshot> {
    let path = manager::homes_dir().join(&account.uuid).join("auth.json");
    let credentials = auth::load(&path)?;
    let credentials = match api::maybe_refresh(client, &credentials).await? {
        Some(fresh) => {
            auth::save(&fresh, &path).ok();
            fresh
        }
        None => credentials,
    };
    Ok(api::fetch_quota(client, &credentials).await?)
}

fn safe_error(error: &str) -> String {
    // Keep useful status context while preventing response bodies or credentials from entering
    // the UI. The API client includes a short body for HTTP failures.
    let message = if error.starts_with("API HTTP") {
        error.split(": ").next().unwrap_or(error)
    } else {
        error
    };
    message
        .replace("Bearer ", "Bearer [redacted] ")
        .replace("access_token", "token")
        .replace("refresh_token", "token")
        .chars()
        .take(120)
        .collect()
}

fn render(frame: &mut ratatui::Frame<'_>, model: &mut Model) {
    let area = frame.area();
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(2),
        ])
        .split(area);
    let targets = manager::active_targets(&model.accounts);
    let active = model
        .accounts
        .iter()
        .find(|a| targets.get(&a.id).is_some_and(|target| !target.is_empty()));
    let title = Paragraph::new(Line::from(vec![
        Span::styled(
            " CODexCTL ",
            Style::default()
                .fg(Color::LightCyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" v{}  ·  centro de cuentas", env!("CARGO_PKG_VERSION")),
            Style::default().fg(Color::Gray),
        ),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Blue)),
    );
    frame.render_widget(title, layout[0]);
    let available = model.views.values().filter(|v| v.quota.is_some()).count();
    let active_name = active.map(|a| a.nickname.as_str()).unwrap_or("ninguna");
    let active_target = active
        .and_then(|a| targets.get(&a.id))
        .map(|target| format!(" ({})", target_label(Some(target))))
        .unwrap_or_default();
    let summary = Line::from(vec![
        Span::raw(" "),
        Span::styled(
            format!("{} cuentas", model.accounts.len()),
            Style::default()
                .fg(Color::LightCyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ·  ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("{} disponibles", available),
            Style::default()
                .fg(Color::LightGreen)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ·  activa: ", Style::default().fg(Color::Gray)),
        Span::styled(
            format!("{}{}", active_name, active_target),
            Style::default()
                .fg(Color::LightMagenta)
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(summary).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(Span::styled(
                    " Resumen ",
                    Style::default().fg(Color::LightCyan),
                )),
        ),
        layout[1],
    );
    render_accounts(frame, model, layout[2]);
    let spinner = ["·", "··", "···"][(model
        .last_refresh
        .map(|t| t.elapsed().as_millis() / 300)
        .unwrap_or(0) as usize)
        % 3];
    let footer = format!(
        " {}  {}  │  ↑↓ seleccionar  ←→ columnas  Enter/s activar  a reautenticar  r actualizar  q/Esc salir",
        if model.loading { spinner } else { "✓" },
        model.status
    );
    frame.render_widget(
        Paragraph::new(footer).style(Style::default().fg(Color::LightYellow)),
        layout[3],
    );
}

fn render_accounts(frame: &mut ratatui::Frame<'_>, model: &mut Model, area: Rect) {
    let compact = area.width < 100;
    let targets = manager::active_targets(&model.accounts);
    let headers = column_headers(compact);
    let all_widths = column_widths(compact);
    let visible = visible_columns(area.width, &all_widths, model.horizontal_offset);
    let headers = visible
        .iter()
        .map(|&index| headers[index])
        .collect::<Vec<_>>();
    let rows = model.accounts.iter().map(|account| {
        let view = model.views.get(&account.id);
        let target = targets.get(&account.id).map(String::as_str);
        let status = if target.is_some_and(|target| !target.is_empty()) {
            "● ACTIVA"
        } else {
            "○ lista"
        };
        let first = if let Some(error) = view.and_then(|v| v.error.as_deref()) {
            if compact {
                format!("{} ⚠ {}", account.nickname, error)
            } else {
                format!("{}  ·  ⚠ ERROR: {}", account.nickname, error)
            }
        } else {
            format!("{} {}", account.nickname, status)
        };
        let destination = target_label(target);
        let q = view.and_then(|v| v.quota.as_ref());
        let cells = vec![
            first,
            email_display(account.email.as_deref(), compact),
            account.plan_type.clone().unwrap_or_else(|| "—".into()),
            destination.to_string(),
            if compact {
                window_compact(q.and_then(|q| q.primary_window.as_ref()))
            } else {
                window(q.and_then(|q| q.primary_window.as_ref()))
            },
            if compact {
                window_compact(q.and_then(|q| q.secondary_window.as_ref()))
            } else {
                window(q.and_then(|q| q.secondary_window.as_ref()))
            },
            free(q),
            reset(q),
        ];
        let has_error = view.and_then(|v| v.error.as_ref()).is_some();
        let cells = visible.iter().map(|&index| {
            let value = cells[index].clone();
            let style = if has_error {
                error_style()
            } else if compact {
                match index {
                    3 => destination_style(target),
                    4 => window_style(q.and_then(|q| q.primary_window.as_ref())),
                    5 => window_style(q.and_then(|q| q.secondary_window.as_ref())),
                    6 => free_style(q),
                    7 => reset_style(q),
                    _ => Style::default(),
                }
            } else {
                match index {
                    0 => Style::default().fg(Color::White),
                    1 | 2 => Style::default().fg(Color::Gray),
                    3 => destination_style(target),
                    4 => window_style(q.and_then(|q| q.primary_window.as_ref())),
                    5 => window_style(q.and_then(|q| q.secondary_window.as_ref())),
                    6 => free_style(q),
                    7 => reset_style(q),
                    _ => Style::default(),
                }
            };
            Cell::from(value).style(style)
        });
        Row::new(cells).style(if has_error {
            error_style()
        } else {
            Style::default()
        })
    });
    let widths = visible
        .iter()
        .map(|&index| Constraint::Length(all_widths[index]))
        .collect::<Vec<_>>();
    let title = if visible.len() < all_widths.len() {
        format!(
            " Cuentas · columnas {}–{}/{} · ←→ ",
            visible[0] + 1,
            visible[visible.len() - 1] + 1,
            all_widths.len()
        )
    } else {
        " Cuentas ".into()
    };
    let table = Table::new(rows, widths)
        .header(
            Row::new(headers).style(
                Style::default()
                    .fg(Color::LightCyan)
                    .add_modifier(Modifier::BOLD),
            ),
        )
        .column_spacing(1)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Blue))
                .title(Span::styled(title, Style::default().fg(Color::LightCyan))),
        )
        .row_highlight_style(Style::default().bg(Color::Blue).fg(Color::White))
        .highlight_symbol("▶ ");
    frame.render_stateful_widget(table, area, &mut model.table);
}

fn column_headers(compact: bool) -> [&'static str; 8] {
    if compact {
        [
            "CUENTA",
            "EMAIL",
            "PLAN",
            "DEST.",
            "5h",
            "7d",
            "REINICIOS",
            "PRÓX.",
        ]
    } else {
        [
            "CUENTA",
            "EMAIL",
            "PLAN",
            "DESTINO",
            "5h USADO",
            "7d USADO",
            "REINICIOS",
            "PRÓXIMO REINICIO",
        ]
    }
}

fn column_widths(compact: bool) -> [u16; 8] {
    if compact {
        [20, 18, 10, 11, 7, 7, 10, 14]
    } else {
        [22, 24, 10, 10, 12, 12, 14, 20]
    }
}

fn visible_columns<const N: usize>(
    area_width: u16,
    widths: &[u16; N],
    offset: usize,
) -> Vec<usize> {
    let available = area_width.saturating_sub(2) as usize;
    let mut used = 0usize;
    let mut visible = Vec::new();
    for index in offset.min(widths.len() - 1)..widths.len() {
        let needed = widths[index] as usize + usize::from(!visible.is_empty());
        if !visible.is_empty() && used + needed > available {
            break;
        }
        visible.push(index);
        used += needed;
    }
    visible
}

fn email_display(email: Option<&str>, compact: bool) -> String {
    let email = email.filter(|email| !email.is_empty()).unwrap_or("—");
    if compact {
        truncate(email, 16)
    } else {
        email.to_owned()
    }
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }
    let truncated: String = value.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{truncated}…")
}

fn window(window: Option<&UsageWindow>) -> String {
    window
        .map(|w| format!("{:.0}% usado", w.used_percent))
        .unwrap_or_else(|| "—".into())
}

fn window_compact(window: Option<&UsageWindow>) -> String {
    window
        .map(|w| format!("{:.0}%", w.used_percent))
        .unwrap_or_else(|| "—".into())
}

fn window_style(window: Option<&UsageWindow>) -> Style {
    match window.map(|w| w.used_percent) {
        Some(percent) if percent >= 90.0 => Style::default().fg(Color::LightRed),
        Some(percent) if percent >= 70.0 => Style::default().fg(Color::LightYellow),
        Some(_) => Style::default().fg(Color::LightGreen),
        None => Style::default().fg(Color::DarkGray),
    }
}

fn destination_style(target: Option<&str>) -> Style {
    let color = match target {
        Some("codex") => Color::LightBlue,
        Some("opencode") => Color::LightMagenta,
        Some("both") => Color::LightGreen,
        _ => Color::DarkGray,
    };
    Style::default().fg(color).add_modifier(Modifier::BOLD)
}

fn free_style(quota: Option<&QuotaSnapshot>) -> Style {
    match quota.and_then(|q| q.rate_limit_reset_credits.as_ref()) {
        Some(credits) if credits.applicable_available_count > 0 => {
            Style::default().fg(Color::LightGreen)
        }
        Some(_) => Style::default().fg(Color::LightRed),
        None => Style::default().fg(Color::DarkGray),
    }
}

fn reset_style(quota: Option<&QuotaSnapshot>) -> Style {
    if quota
        .and_then(|q| q.primary_window.as_ref())
        .and_then(|window| window.reset_at)
        .is_some()
    {
        Style::default().fg(Color::LightMagenta)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

fn error_style() -> Style {
    Style::default()
        .fg(Color::LightRed)
        .add_modifier(Modifier::BOLD)
}

fn target_label(target: Option<&str>) -> &'static str {
    match target {
        Some("codex") => "Codex",
        Some("opencode") => "OpenCode",
        Some("both") => "Ambos",
        _ => "Ninguno",
    }
}
fn free(quota: Option<&QuotaSnapshot>) -> String {
    quota
        .and_then(|q| q.rate_limit_reset_credits.as_ref())
        .map(|c| format!("{}/{}", c.applicable_available_count, c.available_count))
        .unwrap_or_else(|| "—".into())
}
fn reset(quota: Option<&QuotaSnapshot>) -> String {
    quota
        .and_then(|q| q.primary_window.as_ref())
        .and_then(|w| w.reset_at)
        .map(|d| {
            let local = d.with_timezone(&chrono::Local);
            local.format("%d/%m %H:%M").to_string()
        })
        .unwrap_or_else(|| "—".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_layout_keeps_all_core_columns() {
        assert_eq!(column_headers(true).len(), 8);
        assert_eq!(
            column_headers(true)[..4],
            ["CUENTA", "EMAIL", "PLAN", "DEST."]
        );
        assert_eq!(
            column_headers(true)[4..],
            ["5h", "7d", "REINICIOS", "PRÓX."]
        );
        assert_eq!(
            email_display(Some("person@example.com"), true),
            "person@example.…"
        );
        assert_eq!(
            email_display(Some("person@example.com"), false),
            "person@example.com"
        );
        assert_eq!(
            window_compact(Some(&UsageWindow {
                used_percent: 42.0,
                reset_at: None,
                limit_window_seconds: 300,
            })),
            "42%"
        );
    }

    #[test]
    fn narrow_layout_exposes_overflow_as_scrollable_column_ranges() {
        let widths = column_widths(true);
        assert_eq!(visible_columns(50, &widths, 0), vec![0, 1]);
        assert_eq!(visible_columns(50, &widths, 1), vec![1, 2, 3]);
        assert_eq!(visible_columns(40, &widths, 7), vec![7]);
    }

    #[test]
    fn target_labels_are_clear_for_each_activation_state() {
        let cases = [
            (Some("codex"), "Codex"),
            (Some("opencode"), "OpenCode"),
            (Some("both"), "Ambos"),
            (Some(""), "Ninguno"),
            (None, "Ninguno"),
        ];
        for (target, expected) in cases {
            assert_eq!(target_label(target), expected);
        }
    }

    #[test]
    fn semantic_styles_cover_destination_and_quota_health() {
        let window = |used_percent| UsageWindow {
            used_percent,
            reset_at: None,
            limit_window_seconds: 300,
        };
        let cases = [
            (Some("codex"), Color::LightBlue),
            (Some("opencode"), Color::LightMagenta),
            (Some("both"), Color::LightGreen),
            (Some(""), Color::DarkGray),
        ];
        for (target, color) in cases {
            assert_eq!(destination_style(target).fg, Some(color));
        }
        assert_eq!(
            window_style(Some(&window(20.0))).fg,
            Some(Color::LightGreen)
        );
        assert_eq!(
            window_style(Some(&window(70.0))).fg,
            Some(Color::LightYellow)
        );
        assert_eq!(window_style(Some(&window(90.0))).fg, Some(Color::LightRed));
        assert_eq!(window_style(None).fg, Some(Color::DarkGray));
    }

    #[test]
    fn auto_refresh_waits_for_interval_and_loading_to_finish() {
        let start = Instant::now();
        assert!(!auto_refresh_due(
            start,
            start + AUTO_REFRESH_INTERVAL,
            false
        ));
        assert!(!auto_refresh_due(
            start + AUTO_REFRESH_INTERVAL,
            start,
            true
        ));
        assert!(auto_refresh_due(
            start + AUTO_REFRESH_INTERVAL,
            start,
            false
        ));
    }

    #[test]
    fn normal_layout_keeps_email_between_account_and_plan() {
        assert_eq!(
            column_headers(false),
            [
                "CUENTA",
                "EMAIL",
                "PLAN",
                "DESTINO",
                "5h USADO",
                "7d USADO",
                "REINICIOS",
                "PRÓXIMO REINICIO",
            ]
        );
        assert_eq!(column_widths(false).len(), 8);
    }

    #[test]
    fn selection_wraps_in_both_directions() {
        let accounts = vec![
            Account {
                id: "a".into(),
                nickname: "A".into(),
                uuid: "a".into(),
                email: None,
                plan_type: None,
                provider_account_id: None,
                user_id: None,
                created_at: String::new(),
                updated_at: String::new(),
            },
            Account {
                id: "b".into(),
                nickname: "B".into(),
                uuid: "b".into(),
                email: None,
                plan_type: None,
                provider_account_id: None,
                user_id: None,
                created_at: String::new(),
                updated_at: String::new(),
            },
        ];
        let mut model = Model::new(accounts);
        model.move_selection(-1);
        assert_eq!(model.selected, 1);
        model.move_selection(1);
        assert_eq!(model.selected, 0);
    }
}
