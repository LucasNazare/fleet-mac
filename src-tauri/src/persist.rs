//! JSON persistence in ~/Library/Application Support/fleet/.
use crate::model::{Project, Session};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// ~/Library/Application Support/fleet, or $FLEET_APP_DIR (a second "Mac" on one machine).
pub fn app_dir() -> PathBuf {
    let d = std::env::var_os("FLEET_APP_DIR").map(PathBuf::from).unwrap_or_else(|| dirs::data_dir().unwrap_or_else(|| PathBuf::from("/tmp")).join("fleet"));
    let _ = std::fs::create_dir_all(&d);
    d
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Store {
    #[serde(default)]
    pub projects: Vec<Project>,
    #[serde(default)]
    pub sessions: Vec<Session>,
    /// Processes the user chose to leave running when fleet last quit.
    #[serde(default)]
    pub detached: Vec<crate::processes::ProcessRun>,
}

fn write_atomic(name: &str, data: &[u8]) -> std::io::Result<()> {
    let dir = app_dir();
    let tmp = dir.join(format!(".{name}.tmp"));
    std::fs::write(&tmp, data)?;
    std::fs::rename(tmp, dir.join(name))
}

pub fn load_store() -> Store {
    std::fs::read(app_dir().join("state.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn save_store(store: &Store) {
    if let Ok(b) = serde_json::to_vec_pretty(store) {
        let _ = write_atomic("state.json", &b);
    }
}

pub fn load_ui() -> serde_json::Value {
    std::fs::read(app_dir().join("ui.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(serde_json::Value::Null)
}

pub fn save_ui(v: &serde_json::Value) {
    if let Ok(b) = serde_json::to_vec_pretty(v) {
        let _ = write_atomic("ui.json", &b);
    }
}
