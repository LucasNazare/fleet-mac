//! settings.json in the app-support folder. The frontend owns the schema
//! (src/store/settings.ts); the backend reads the few keys it needs.
use parking_lot::Mutex;
use serde_json::Value;
use std::sync::OnceLock;

static SETTINGS: OnceLock<Mutex<Value>> = OnceLock::new();

fn cell() -> &'static Mutex<Value> {
    SETTINGS.get_or_init(|| {
        let v = std::fs::read(crate::persist::app_dir().join("settings.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or(Value::Null);
        Mutex::new(v)
    })
}

fn get(path: &[&str]) -> Value {
    let s = cell().lock();
    let mut v = &*s;
    for k in path {
        v = match v.get(*k) {
            Some(x) => x,
            None => return Value::Null,
        };
    }
    v.clone()
}

pub fn claude_path() -> Option<String> {
    get(&["claude", "path"]).as_str().map(str::trim).filter(|s| !s.is_empty()).map(String::from)
}

/// Default args, split like a shell would for simple cases (quotes supported).
pub fn default_args() -> Vec<String> {
    split_args(get(&["claude", "defaultArgs"]).as_str().unwrap_or(""))
}

pub fn split_args(s: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut any = false;
    for c in s.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                any = true;
            }
            (None, c) if c.is_whitespace() => {
                if !cur.is_empty() || any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            (None, c) => cur.push(c),
        }
    }
    if !cur.is_empty() || any {
        out.push(cur);
    }
    out
}

pub fn default_permission_mode() -> Option<String> {
    get(&["claude", "permissionMode"]).as_str().filter(|s| !s.is_empty() && *s != "default").map(String::from)
}

pub fn title_model() -> String {
    get(&["claude", "model"]).as_str().filter(|s| !s.is_empty()).unwrap_or("haiku").to_string()
}

pub fn auto_titles() -> bool {
    get(&["claude", "autoTitles"]).as_bool().unwrap_or(true)
}

pub fn muted(project_id: &str, session_id: &str) -> bool {
    let has = |k: &str, id: &str| get(&["notifications", k]).as_array().map(|a| a.iter().any(|v| v.as_str() == Some(id))).unwrap_or(false);
    has("mutedProjects", project_id) || has("mutedSessions", session_id)
}

/// Session palette from settings (only if it's a full set of #RRGGBB colours).
pub fn palette() -> Vec<String> {
    let v: Vec<String> = get(&["appearance", "palette"]).as_array().map(|a| a.iter().filter_map(|c| c.as_str().map(String::from)).collect()).unwrap_or_default();
    let valid = v.len() >= 6 && v.iter().all(|c| c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|x| x.is_ascii_hexdigit()));
    if valid { v } else { crate::identity::PALETTE.iter().map(|s| s.to_string()).collect() }
}

pub fn sounds() -> bool {
    get(&["notifications", "sounds"]).as_bool().unwrap_or(false)
}

pub fn menu_bar() -> bool {
    get(&["notifications", "menuBar"]).as_bool().unwrap_or(false)
}

/// The UI theme id (settings.json appearance.theme).
pub fn theme() -> String {
    get(&["appearance", "theme"]).as_str().unwrap_or("dark").to_string()
}

pub fn sync_paused_all() -> bool {
    get(&["sync", "pausedAll"]).as_bool().unwrap_or(false)
}

pub fn set_sync_paused_all(paused: bool) {
    let mut s = cell().lock();
    if !s.is_object() {
        *s = serde_json::json!({});
    }
    let obj = s.as_object_mut().unwrap();
    let sync = obj.entry("sync").or_insert_with(|| serde_json::json!({}));
    if !sync.is_object() {
        *sync = serde_json::json!({});
    }
    sync.as_object_mut().unwrap().insert("pausedAll".into(), Value::Bool(paused));
    let _ = std::fs::write(crate::persist::app_dir().join("settings.json"), serde_json::to_vec_pretty(&*s).unwrap());
}

#[tauri::command]
pub fn get_settings() -> Value {
    cell().lock().clone()
}

#[tauri::command]
pub fn save_settings(app: tauri::AppHandle, settings: Value) -> Result<(), String> {
    let path = crate::persist::app_dir().join("settings.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&settings).unwrap()).map_err(|e| e.to_string())?;
    *cell().lock() = settings;
    crate::tray::refresh(&app);
    Ok(())
}

#[tauri::command]
pub fn reveal_settings() -> Result<(), String> {
    let path = crate::persist::app_dir().join("settings.json");
    if !path.exists() {
        std::fs::write(&path, b"{}\n").map_err(|e| e.to_string())?;
    }
    std::process::Command::new("/usr/bin/open").arg("-R").arg(&path).spawn().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_args() {
        assert_eq!(split_args("--model opus  --add-dir '/a b' \"\""), vec!["--model", "opus", "--add-dir", "/a b", ""]);
        assert!(split_args("   ").is_empty());
    }
}
