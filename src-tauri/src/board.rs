//! Rattleboard (SPEC.md §8.4): connecting a board, sending it the notifications the alert rules
//! raise, and keeping the AI runway card in its rotation up to date. The text and the calls are
//! in tokenrunway_core::rattleboard; this holds the state between them and the commands.
//!
//! Nothing here ever pops up: what happened (or went wrong) is one status line in Settings
//! (`AppView.rattleboardStatus`) and the debug log. The key lives only in the keychain, and is
//! never logged or put in an error.

use crate::state::{broadcast, AppState};
use serde::Serialize;
use tauri::{AppHandle, Manager};
use tokenrunway_core::allowance::{self, alerts::Alert, secrets};
use tokenrunway_core::now_ms;
use tokenrunway_core::rattleboard::{self as rb, Board, Note};
use tokenrunway_core::view::{AccountView, AppView, RattleboardSettings, Status};

/// What the app knows about the board between calls.
#[derive(Debug, Default)]
pub struct Runtime {
    pub status: Option<String>,
    /// The keychain entry, read once (None = not read yet)
    key: Option<Option<String>>,
    /// The board has /rattleboard/notify (None = not asked yet: try it, falling back on a 404)
    notify: Option<bool>,
    card: Card,
}

/// The card as last sent.
#[derive(Debug, Default)]
struct Card {
    text: String,
    at: i64,
    crit: bool,
    /// Checked it's in the rotation this session
    ensured: bool,
    busy: bool,
}

fn clock() -> String {
    chrono::Local::now().format("%H:%M").to_string()
}

fn settings(app: &AppHandle) -> RattleboardSettings {
    app.state::<AppState>().settings.lock().unwrap().rattleboard.clone()
}

/// The board's key, from the keychain (read once, only once a board is connected).
pub fn key(app: &AppHandle) -> Option<String> {
    let st = app.state::<AppState>();
    let mut b = st.board.lock().unwrap();
    if b.key.is_none() {
        b.key = Some(secrets::get(rb::KEY_NAME));
    }
    b.key.clone().flatten()
}

/// The API to call and its key, when there is one: the board's, or this computer's server
/// when the key was given too.
fn api(app: &AppHandle, s: &RattleboardSettings) -> Option<(String, String)> {
    let url = match s.mode.as_str() {
        "network" if !s.url.is_empty() => s.url.clone(),
        "local" => rb::local_url(),
        _ => return None,
    };
    Some((url, key(app)?))
}

/// For the view: whether the key is there (never reads the keychain while nothing's connected).
pub fn has_key(app: &AppHandle, s: &RattleboardSettings) -> bool {
    s.mode != "off" && key(app).is_some()
}

pub fn status(app: &AppHandle) -> Option<String> {
    app.state::<AppState>().board.lock().unwrap().status.clone()
}

fn set_status(app: &AppHandle, msg: Option<String>) {
    if let Some(m) = &msg {
        crate::debug(&format!("rattleboard: {m}"));
    }
    app.state::<AppState>().board.lock().unwrap().status = msg;
    broadcast(app);
}

/// Sends one note the way the settings say: the API when there's a key (and no `--target`),
/// else the `rattleboard --notify` CLI on this computer.
async fn send(app: &AppHandle, s: &RattleboardSettings, note: &Note, seconds: u64) -> Result<(), String> {
    let use_cli = s.mode == "local" && !s.target.trim().is_empty();
    match api(app, s).filter(|_| !use_cli) {
        Some((url, key)) => {
            let http = app.state::<AppState>().ctx.http.clone();
            let has_notify = app.state::<AppState>().board.lock().unwrap().notify.unwrap_or(true);
            rb::notify(&http, &url, &key, note, seconds, has_notify, (s.rows as usize, s.cols as usize)).await
        }
        None if s.mode == "local" => {
            let bin = rb::find_binary().ok_or("Rattleboard isn't installed on this computer any more")?;
            rb::notify_cli(&bin, note, seconds, &s.target).await
        }
        None if s.mode == "network" => Err("the board's key is missing. Connect it again with its pair link".into()),
        None => Err("no board is connected".into()),
    }
}

/// The provider's usage page for an account, the link that goes with its alert.
fn link_for(app: &AppHandle, account_id: &str) -> Option<String> {
    let rec = app.state::<AppState>().accounts.lock().unwrap().get(account_id).cloned()?;
    allowance::provider_for(&rec).map(|p| p.usage_url(&rec))
}

/// The notifications the rules just raised (pause and once-per-window already applied), sent
/// to the board when its level wants them.
pub fn on_alerts(app: &AppHandle, fired: &[Alert], accounts: &[AccountView]) {
    let s = settings(app);
    if s.mode == "off" || s.alerts == "off" || fired.is_empty() {
        return;
    }
    let now = now_ms();
    let notes: Vec<Note> = fired.iter().filter_map(|a| rb::note_for(a, accounts, &s.alerts, link_for(app, &a.account_id), now)).collect();
    if notes.is_empty() {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut sent = 0;
        let mut error = None;
        for n in &notes {
            match send(&app, &s, n, rb::ALERT_SECONDS).await {
                Ok(()) => sent += 1,
                Err(e) => {
                    error = Some(e);
                    break;
                }
            }
        }
        let msg = match error {
            Some(e) => format!("Couldn't send an alert at {}: {e}", clock()),
            None if sent == 1 => format!("Sent an alert at {}", clock()),
            None => format!("Sent {sent} alerts at {}", clock()),
        };
        set_status(&app, Some(msg));
    });
}

/// Keeps the card up to date: re-sent when its text changes, at most every 5 minutes, and
/// straight away when the shortest runway turns critical. Called with every view.
pub fn card_tick(app: &AppHandle, view: &AppView) {
    let s = &view.settings.rattleboard;
    if s.mode == "off" || !s.card || s.kind == "vestaboard" {
        return;
    }
    let Some((url, key)) = api(app, s) else { return };
    let now = now_ms();
    let text = rb::card_text(&view.accounts, view.summary.as_ref(), s.rows as usize, s.cols as usize, now);
    let crit = view.summary.as_ref().is_some_and(|x| x.status == Status::Crit);
    let ensure = {
        let st = app.state::<AppState>();
        let mut b = st.board.lock().unwrap();
        let c = &mut b.card;
        let due = c.text != text && (c.at == 0 || now - c.at >= rb::CARD_EVERY_MS || (crit && !c.crit));
        if c.busy || !due {
            return;
        }
        c.busy = true;
        !c.ensured
    };
    let manifest = rb::manifest(&text, rb::card_chip(&view.accounts));
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let http = app.state::<AppState>().ctx.http.clone();
        let r = rb::push_card(&http, &url, &key, &manifest, ensure).await;
        let was_error = {
            let st = app.state::<AppState>();
            let mut b = st.board.lock().unwrap();
            let c = &mut b.card;
            c.busy = false;
            c.at = now;
            // A failure waits the 5 minutes too, even if it's critical: no retry loop.
            c.crit = crit;
            if r.is_ok() {
                c.text = text;
                c.ensured = true;
            }
            b.status.as_deref().is_some_and(|m| m.starts_with("Couldn't update the card"))
        };
        match r {
            Ok(true) => set_status(&app, Some(format!("Added the AI runway card to the rotation at {}", clock()))),
            Ok(false) if was_error => set_status(&app, Some(format!("Updated the card at {}", clock()))),
            Ok(false) => crate::debug("rattleboard: card updated"),
            Err(e) => set_status(&app, Some(format!("Couldn't update the card at {}: {e}", clock()))),
        }
    });
}

/// After `set_settings`: the card switched on (send it now) or off (take it off the board).
pub fn settings_changed(app: &AppHandle, before: &RattleboardSettings, after: &RattleboardSettings) {
    if before.card == after.card {
        return;
    }
    app.state::<AppState>().board.lock().unwrap().card = Card::default();
    if !after.card {
        if let Some((url, key)) = api(app, after) {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                let http = app.state::<AppState>().ctx.http.clone();
                match rb::remove_card(&http, &url, &key).await {
                    Ok(()) => set_status(&app, Some(format!("Took the card off the board at {}", clock()))),
                    Err(e) => set_status(&app, Some(format!("Couldn't remove the card: {e}"))),
                }
            });
        }
    }
}

/// Whether the card can be switched on (set_settings refuses it otherwise).
pub fn card_possible(app: &AppHandle, s: &RattleboardSettings) -> Result<(), String> {
    if s.kind == "vestaboard" {
        return Err("A Vestaboard has no rotation for the card; it's for Rattleboards.".into());
    }
    if api(app, s).is_none() {
        return Err("The card needs the board's key: paste its pair link from Rattleboard → Settings → API server.".into());
    }
    Ok(())
}

// ---------------------------------------------------------------- commands

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Detect {
    /// The app is installed and its API server answers
    pub found: bool,
    pub binary: bool,
    pub server: bool,
}

#[tauri::command]
pub async fn rattleboard_detect(app: AppHandle) -> Detect {
    let binary = rb::find_binary().is_some();
    let http = app.state::<AppState>().ctx.http.clone();
    let server = rb::server_there(&http, &rb::local_url()).await;
    Detect { found: binary && server, binary, server }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Connected {
    pub name: String,
    pub rows: u32,
    pub cols: u32,
    /// What the Test said
    pub message: Option<String>,
}

const THIS_COMPUTER: &str = "This computer";

fn nonempty(s: Option<String>) -> Option<String> {
    s.map(|x| x.trim().to_string()).filter(|x| !x.is_empty())
}

/// Connects a board: "local" (the app here; a key or pair link is optional, for the card) or
/// "network" (a pair link, or an address and key). Checks it answers before saving anything.
#[tauri::command]
pub async fn rattleboard_connect(
    app: AppHandle,
    mode: String,
    link: Option<String>,
    url: Option<String>,
    key: Option<String>,
) -> Result<Connected, String> {
    let http = app.state::<AppState>().ctx.http.clone();
    let pair = nonempty(link).map(|l| rb::parse_pair(&l)).transpose()?;
    let key = pair.as_ref().map(|p| p.key.clone()).or(nonempty(key));
    let (base, board) = match mode.as_str() {
        "local" => {
            let base = rb::local_url();
            match &key {
                Some(k) => match rb::probe(&http, &base, k).await? {
                    b @ Board::Rattleboard(_) => (base, Some(b)),
                    Board::Vestaboard { .. } => return Err("That isn't this computer's Rattleboard.".into()),
                },
                None if rb::find_binary().is_none() => {
                    return Err("Rattleboard isn't installed on this computer. Get it from github.com/slastrina/rattleboard.".into())
                }
                None => (base, None),
            }
        }
        "network" => {
            let base = match &pair {
                Some(p) => p.url.clone(),
                None => rb::normalize_url(&url.unwrap_or_default())?,
            };
            let k = key.as_deref().ok_or("Paste the board's pair link, or its key.")?;
            (base.clone(), Some(rb::probe(&http, &base, k).await?))
        }
        other => return Err(format!("Unknown mode {other}")),
    };
    match &key {
        Some(k) => secrets::set(rb::KEY_NAME, k)?,
        None => secrets::delete(rb::KEY_NAME),
    }
    let (name, (rows, cols)) = match &board {
        Some(b) => (b.name(), b.size()),
        None => (THIS_COMPUTER.to_string(), (6, 22)),
    };
    let st = app.state::<AppState>();
    {
        let mut b = st.board.lock().unwrap();
        *b = Runtime { key: Some(key.clone()), notify: board.as_ref().map(Board::notify), ..Default::default() };
    }
    {
        let mut s = st.settings.lock().unwrap();
        let r = &mut s.rattleboard;
        r.mode = mode.clone();
        r.url = base.clone();
        r.name = name.clone();
        r.rows = rows as u32;
        r.cols = cols as u32;
        r.kind = match &board {
            Some(Board::Vestaboard { .. }) => "vestaboard".into(),
            _ => "rattleboard".into(),
        };
        if r.alerts.is_empty() {
            r.alerts = "alerts".into();
        }
        r.card = r.card && key.is_some() && r.kind == "rattleboard";
    }
    st.save_settings();
    crate::debug(&format!("rattleboard: connected {mode} {} ({rows}x{cols}), key {}", rb::host_of(&base), key.is_some()));
    st.board.lock().unwrap().status = None;
    broadcast(&app);
    Ok(Connected { name, rows: rows as u32, cols: cols as u32, message: None })
}

/// Sends "TOKENRUNWAY CONNECTED" for 15 s, and with the key reads the board's name and size.
#[tauri::command]
pub async fn rattleboard_test(app: AppHandle) -> Result<Connected, String> {
    let s = settings(&app);
    if s.mode == "off" {
        return Err("Connect a board first.".into());
    }
    let http = app.state::<AppState>().ctx.http.clone();
    let result: Result<Connected, String> = async {
        let (name, rows, cols) = match api(&app, &s) {
            Some((url, key)) => {
                let b = rb::probe(&http, &url, &key).await?;
                let (rows, cols) = b.size();
                app.state::<AppState>().board.lock().unwrap().notify = Some(b.notify());
                (b.name(), rows as u32, cols as u32)
            }
            None => (s.name.clone(), s.rows, s.cols),
        };
        let s2 = RattleboardSettings { rows, cols, ..s.clone() };
        send(&app, &s2, &Note::test(), rb::TEST_SECONDS).await?;
        let how = if api(&app, &s).is_some() && s.target.trim().is_empty() {
            format!("Sent to “{name}” ({rows}×{cols})")
        } else {
            "Sent with rattleboard --notify (exit 0)".to_string()
        };
        Ok(Connected { name, rows, cols, message: Some(how) })
    }
    .await;
    match &result {
        Ok(c) => {
            let st = app.state::<AppState>();
            let changed = {
                let mut set = st.settings.lock().unwrap();
                let r = &mut set.rattleboard;
                let changed = (r.name.as_str(), r.rows, r.cols) != (c.name.as_str(), c.rows, c.cols);
                (r.name, r.rows, r.cols) = (c.name.clone(), c.rows, c.cols);
                changed
            };
            if changed {
                st.save_settings();
                st.board.lock().unwrap().card = Card::default();
            }
            set_status(&app, Some(format!("{} at {}", c.message.clone().unwrap_or_default(), clock())));
        }
        Err(e) => set_status(&app, Some(format!("Couldn't send the test at {}: {e}", clock()))),
    }
    result
}

/// Forgets the board: takes the card off it (best effort), and the key out of the keychain.
#[tauri::command]
pub async fn rattleboard_disconnect(app: AppHandle) -> Result<(), String> {
    let s = settings(&app);
    if s.card {
        if let Some((url, key)) = api(&app, &s) {
            let http = app.state::<AppState>().ctx.http.clone();
            if let Err(e) = rb::remove_card(&http, &url, &key).await {
                crate::debug(&format!("rattleboard: removing the card: {e}"));
            }
        }
    }
    secrets::delete(rb::KEY_NAME);
    let st = app.state::<AppState>();
    *st.board.lock().unwrap() = Runtime { key: Some(None), ..Default::default() };
    st.settings.lock().unwrap().rattleboard = RattleboardSettings::default();
    st.save_settings();
    crate::debug("rattleboard: disconnected");
    broadcast(&app);
    Ok(())
}
