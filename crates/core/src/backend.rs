use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct HostStatus {
    pub installed: bool,
    pub version: String,
    pub program: String,
    #[serde(rename = "dataPath")]
    pub data_path: String,
    #[serde(rename = "detectionSource")]
    pub detection_source: String,
    pub instance: String,
    pub ready: bool,
    pub problem: Option<String>,
    pub running: bool,
    #[serde(rename = "playerPid")]
    pub player_pid: u32,
    #[serde(rename = "maxWindows")]
    pub max_windows: usize,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct AndroidApp {
    pub package: String,
    pub name: String,
    pub activity: String,
    pub system: bool,
    pub enabled: bool,
    pub icon: String,
    pub registered: bool,
    #[serde(rename = "gameId")]
    pub game_id: String,
    pub promotional: bool,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Snapshot {
    pub status: HostStatus,
    pub apps: Vec<AndroidApp>,
    pub source: String,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct CleanupChange {
    pub package: String,
    pub operation: String,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct CleanupPlan {
    pub id: String,
    pub changes: Vec<CleanupChange>,
    pub items: Vec<BloatwareItem>,
    pub data_path: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BloatwareStatus {
    Cleared,
    Pending,
    Blocked,
    Failed,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BloatwareItem {
    pub package: String,
    pub name: String,
    pub status: BloatwareStatus,
    pub reason: String,
    pub can_clean: bool,
}
#[derive(Clone)]
pub enum Job {
    Scan,
    Connect,
    Stop,
    Register(AndroidApp),
    Launch(AndroidApp),
    LoadBloatware,
    Apply(String),
    CleanPackage(String, String),
    Undo,
    SetupCore,
    RestoreCore,
    SelectInstallation(PathBuf),
    SelectData(PathBuf, String),
    AutoDetect,
}
pub struct Outcome {
    pub snapshot: Option<Snapshot>,
    pub plan: Option<CleanupPlan>,
    pub message: String,
}
#[derive(Clone, Debug)]
pub struct Backend {
    pub root: PathBuf,
}

pub fn validate_package(package: &str) -> Result<()> {
    let valid = regex::Regex::new(r"^[A-Za-z][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)+$").unwrap();
    if !valid.is_match(package) {
        bail!("Invalid Android package");
    }
    Ok(())
}
pub fn icon_exists(app: &AndroidApp) -> bool {
    !app.icon.is_empty() && Path::new(&app.icon).is_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn package_input_cannot_be_a_shell_command() {
        assert!(validate_package("com.example.game").is_ok());
        for invalid in [
            "com.example;whoami",
            "com.example & calc",
            "../com.example",
            "a/b",
            "",
        ] {
            assert!(validate_package(invalid).is_err());
        }
    }
    #[test]
    fn scan_preserves_disabled_and_system_flags() {
        let app:AndroidApp=serde_json::from_str(r#"{"package":"com.example.arcade","enabled":false,"system":true,"registered":true,"gameId":"Arcade"}"#).unwrap();
        assert!(!app.enabled);
        assert!(app.system);
        assert_eq!(app.game_id, "Arcade");
    }
}
