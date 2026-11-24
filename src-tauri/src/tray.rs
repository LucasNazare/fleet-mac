//! Optional menu-bar item listing sessions by state (Settings → Notifications).
use crate::model::SessionState;
use crate::Fleet;
use parking_lot::Mutex;
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::{TrayIcon, TrayIconBuilder};
use tauri::{AppHandle, Emitter, Manager};

static TRAY: Mutex<Option<TrayIcon>> = Mutex::new(None);
static LAST: Mutex<String> = Mutex::new(String::new());

/// Create, update or remove the tray to match settings and session states.
pub fn refresh(app: &AppHandle) {
    if !crate::settings::menu_bar() {
        if let Some(t) = TRAY.lock().take() {
            let _ = t.set_visible(false);
        }
        LAST.lock().clear();
        return;
    }
    let f = app.state::<Fleet>();
    let mut rows: Vec<(String, String, SessionState)> = f
        .store
        .lock()
        .sessions
        .iter()
        .filter(|s| !s.archived)
        .map(|s| (s.id.clone(), format!("{} {} · {}", s.state.symbol(), s.glyph, s.title), s.state))
        .collect();
    let rank = |s: SessionState| match s {
        SessionState::NeedsPermission | SessionState::NeedsReply => 0,
        SessionState::Failed => 1,
        SessionState::Done => 2,
        SessionState::Working => 3,
        _ => 4,
    };
    rows.sort_by_key(|r| rank(r.2));
    let sig = rows.iter().map(|r| r.1.as_str()).collect::<Vec<_>>().join("\n");
    if *LAST.lock() == sig && TRAY.lock().is_some() {
        return;
    }
    *LAST.lock() = sig;
    let mut mb = MenuBuilder::new(app);
    if rows.is_empty() {
        if let Ok(i) = MenuItemBuilder::with_id("none", "No sessions").enabled(false).build(app) {
            mb = mb.item(&i);
        }
    }
    for (id, label, _) in &rows {
        if let Ok(i) = MenuItemBuilder::with_id(format!("s:{id}"), label).build(app) {
            mb = mb.item(&i);
        }
    }
    let Ok(menu) = mb.separator().text("show", "Show fleet").build() else { return };
    let needs = rows.iter().filter(|r| rank(r.2) == 0).count();
    let title = if needs > 0 { format!("● {needs}") } else { "fleet".into() };
    let mut tray = TRAY.lock();
    if let Some(t) = tray.as_ref() {
        let _ = t.set_menu(Some(menu));
        let _ = t.set_title(Some(&title));
        let _ = t.set_visible(true);
        return;
    }
    let built = TrayIconBuilder::with_id("fleet")
        .title(&title)
        .menu(&menu)
        .on_menu_event(|app, ev| {
            let id = ev.id().as_ref().to_string();
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
            if let Some(sid) = id.strip_prefix("s:") {
                let _ = app.emit("focus-session", sid);
            }
        })
        .build(app);
    if let Ok(t) = built {
        *tray = Some(t);
    }
}
