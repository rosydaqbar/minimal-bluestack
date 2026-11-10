use crate::{
    backend::{BloatwareItem, BloatwareStatus, CleanupPlan},
    engine::Engine,
    storage,
};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
};
pub const ALLOWED: &[&str] = &[
    "com.bluestacks.gamecenter",
    "com.bluestacks.bsxlauncher",
    "com.bluestacks.nowgg",
    "com.bluestacks.piggy",
    "com.bluestacks.quest",
    "gg.now.billing.service2",
    "com.bluestacks.billing.service",
    "gg.now.billing.service",
    "gg.now.billing.interceptor",
    "gg.now.accounts",
    "com.bluestacks.filemanager",
    "com.bluestacks.settings",
    "com.uncube.launcher3",
];
struct AdComponent {
    target: &'static str,
    name: &'static str,
    kind: &'static str,
}
const AD_COMPONENTS: &[AdComponent] = &[
    AdComponent {
        target: "com.bluestacks.home/com.bluestacks.svc.services.GPLandscapeAdsActivity",
        name: "BS-Services · landscape ads",
        kind: "query-activities",
    },
    AdComponent {
        target: "com.bluestacks.home/com.bluestacks.svc.services.FPShowFullImageActivity",
        name: "BS-Services · full-image promotions",
        kind: "query-activities",
    },
    AdComponent {
        target: "com.bluestacks.home/com.bluestacks.home.AffiliateWebViewActivity",
        name: "BS-Services · affiliate browser",
        kind: "query-activities",
    },
    AdComponent {
        target: "com.bluestacks.home/com.bluestacks.svc.services.AffiliateHandlerHTML_",
        name: "BS-Services · affiliate promotions",
        kind: "query-services",
    },
    AdComponent {
        target: "com.bluestacks.home/com.bluestacks.svc.services.GPAppDownloadStats_",
        name: "BS-Services · download statistics",
        kind: "query-services",
    },
    AdComponent {
        target: "com.bluestacks.home/com.bluestacks.svc.services.CPIOverlayService",
        name: "BS-Services · Play Store ad overlay",
        kind: "query-services",
    },
    AdComponent {
        target: "com.bluestacks.home/com.bluestacks.svc.services.NowggSignInPopupService_",
        name: "BS-Services · now.gg sign-in popup",
        kind: "query-services",
    },
    AdComponent {
        target: "com.bluestacks.home/com.bluestacks.svc.services.ShowPopupForAutoLaunchService",
        name: "BS-Services · auto-launch popup",
        kind: "query-services",
    },
];
fn allowed(target: &str) -> bool {
    ALLOWED.contains(&target) || AD_COMPONENTS.iter().any(|entry| entry.target == target)
}
fn owner(target: &str) -> &str {
    target.split('/').next().unwrap_or(target)
}
fn operation(target: &str) -> &'static str {
    if target.contains('/') {
        "disable-component"
    } else {
        "disable-or-remove-user"
    }
}
#[derive(Clone, Copy)]
enum Privilege {
    Shell,
    Su,
    Unavailable,
}
fn privilege(engine: &Engine) -> Privilege {
    if engine
        .shell(&["id"], 5)
        .is_ok_and(|text| text.starts_with("uid=0("))
    {
        Privilege::Shell
    } else if engine
        .shell(&["/system/xbin/bstk/su", "-c", "id"], 5)
        .is_ok_and(|text| text.starts_with("uid=0("))
    {
        Privilege::Su
    } else {
        Privilege::Unavailable
    }
}
fn privileged_shell(engine: &Engine, args: &[&str]) -> Result<String> {
    match privilege(engine) {
        Privilege::Shell => engine.shell(args, 15),
        Privilege::Su => {
            let command = args
                .iter()
                .map(|arg| format!("'{}'", arg.replace('\'', "'\\''")))
                .collect::<Vec<_>>()
                .join(" ");
            engine.shell(&["/system/xbin/bstk/su", "-c", &command], 15)
        }
        Privilege::Unavailable => bail!(
            "Android rejects vendor component changes under the shell UID. Working Android root access is required for this component."
        ),
    }
}
fn effect(package: &str) -> &'static str {
    match package {
        "com.bluestacks.billing.service"
        | "gg.now.billing.service"
        | "gg.now.billing.service2"
        | "gg.now.billing.interceptor" => {
            "Optional vendor store/payment integration. Disabling it removes BlueStacks/now.gg payment flows; purchases using those flows will be unavailable."
        }
        "gg.now.accounts" => {
            "Optional now.gg account integration. BlueStacks/now.gg account sign-in will be unavailable; Google account packages are kept."
        }
        "com.bluestacks.filemanager" => {
            "Optional Media Manager. Windows file import/export and its media viewer will be unavailable."
        }
        "com.bluestacks.settings" => {
            "Optional BlueStacks settings shortcut and keyboard setup screens. Android's com.android.settings package is kept."
        }
        "com.uncube.launcher3" => {
            "Vendor Home launcher with embedded ad SDKs. Another Home app is selected; this launcher can be disabled."
        }
        _ => {
            "Optional vendor store, cloud or rewards app. Ready to disable; app data will be retained."
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Backup {
    pub id: String,
    pub instance: String,
    #[serde(deserialize_with = "read_changes")]
    pub changes: Vec<String>,
    pub completed: bool,
    #[serde(default)]
    pub undone: bool,
    #[serde(default)]
    pub data_path: String,
}
fn read_changes<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Vec<String>, D::Error> {
    let values = Vec::<serde_json::Value>::deserialize(deserializer)?;
    let mut out = Vec::new();
    for value in values {
        if let Some(package) = value.as_str() {
            out.push(package.to_owned());
        } else {
            if value["wasDisabled"].as_bool() == Some(true) {
                continue;
            }
            if value["operation"].as_str() != Some("disable-user") {
                return Err(serde::de::Error::custom("Invalid legacy cleanup operation"));
            }
            out.push(
                value["package"]
                    .as_str()
                    .ok_or_else(|| serde::de::Error::custom("Missing cleanup package"))?
                    .to_owned(),
            );
        }
    }
    Ok(out)
}
fn packages(text: &str) -> HashSet<String> {
    text.lines()
        .filter_map(|l| l.strip_prefix("package:"))
        .map(|s| s.trim().into())
        .collect()
}
pub fn changes(installed: &HashSet<String>, disabled: &HashSet<String>) -> Vec<String> {
    ALLOWED
        .iter()
        .filter(|p| installed.contains(**p) && !disabled.contains(**p))
        .map(|s| s.to_string())
        .collect()
}
#[derive(Default, Serialize, Deserialize)]
struct Failures {
    data_path: String,
    messages: BTreeMap<String, String>,
}
fn failures(engine: &Engine) -> Failures {
    storage::read::<Failures>(&engine.root.join("state/bloatware-failures.json"))
        .ok()
        .filter(|f| {
            f.data_path
                .eq_ignore_ascii_case(&engine.installation.data_path.to_string_lossy())
        })
        .unwrap_or_else(|| Failures {
            data_path: engine.installation.data_path.to_string_lossy().into(),
            ..Default::default()
        })
}
fn query(engine: &Engine) -> Result<(HashSet<String>, HashSet<String>, Option<String>)> {
    engine.require_android()?;
    let installed = packages(&engine.shell(&["pm", "list", "packages", "--user", "0"], 15)?);
    if !installed.contains("android") {
        bail!("Android's package list is not ready. Try loading bloatware again shortly.")
    }
    let disabled = packages(&engine.shell(&["pm", "list", "packages", "-d", "--user", "0"], 15)?);
    let home = engine
        .shell(
            &[
                "cmd",
                "package",
                "resolve-activity",
                "--brief",
                "--user",
                "0",
                "-a",
                "android.intent.action.MAIN",
                "-c",
                "android.intent.category.HOME",
            ],
            10,
        )
        .ok()
        .and_then(|output| {
            output.lines().find_map(|line| {
                line.trim()
                    .split_once('/')
                    .filter(|(pkg, _)| crate::backend::validate_package(pkg).is_ok())
                    .map(|(pkg, _)| pkg.to_owned())
            })
        });
    Ok((installed, disabled, home))
}
fn vendor(package: &str) -> bool {
    package.starts_with("com.bluestacks.")
        || package.starts_with("gg.now.")
        || package == "com.uncube.launcher3"
}
#[derive(Clone, Copy)]
enum ComponentState {
    Missing,
    Disabled,
    Enabled,
}
fn component_states(
    engine: &Engine,
    installed: &HashSet<String>,
    disabled: &HashSet<String>,
) -> Result<BTreeMap<String, ComponentState>> {
    let mut states = BTreeMap::new();
    for entry in AD_COMPONENTS {
        let state = if !installed.contains(owner(entry.target)) {
            ComponentState::Missing
        } else if disabled.contains(owner(entry.target)) {
            ComponentState::Disabled
        } else {
            let exists = engine.shell(
                &[
                    "cmd",
                    "package",
                    entry.kind,
                    "--components",
                    "--query-flags",
                    "512",
                    "--user",
                    "0",
                    "-n",
                    entry.target,
                ],
                10,
            )?;
            if !component_reply_contains(&exists, entry.target) {
                ComponentState::Missing
            } else {
                let enabled = engine.shell(
                    &[
                        "cmd",
                        "package",
                        entry.kind,
                        "--components",
                        "--user",
                        "0",
                        "-n",
                        entry.target,
                    ],
                    10,
                )?;
                if component_reply_contains(&enabled, entry.target) {
                    ComponentState::Enabled
                } else {
                    ComponentState::Disabled
                }
            }
        };
        states.insert(entry.target.into(), state);
    }
    Ok(states)
}
fn component_reply_contains(reply: &str, target: &str) -> bool {
    reply.lines().any(|line| {
        let line = line.trim();
        if let Some((package, class)) = line.split_once('/') {
            let class = if class.starts_with('.') {
                format!("{package}{class}")
            } else {
                class.into()
            };
            format!("{package}/{class}") == target
        } else {
            false
        }
    })
}
fn component_inventory(
    states: &BTreeMap<String, ComponentState>,
    errors: &BTreeMap<String, String>,
    root_available: bool,
) -> Vec<BloatwareItem> {
    AD_COMPONENTS.iter().map(|entry| {
        let (status, reason, can_clean) = match states.get(entry.target).copied().unwrap_or(ComponentState::Missing) {
            ComponentState::Missing => (BloatwareStatus::Cleared, "Component is not installed in this Android build.".into(), false),
            ComponentState::Disabled => (BloatwareStatus::Cleared, "This component or its parent app is disabled. The shared sign-in package is retained.".into(), false),
            ComponentState::Enabled if !root_available => (BloatwareStatus::Blocked, "Android rejects vendor component changes under the shell UID. Working Android root access is required to disable this component individually; the shared Google sign-in package is kept.".into(), false),
            ComponentState::Enabled => if let Some(error) = errors.get(entry.target) {
                (BloatwareStatus::Failed, format!("Last attempt failed: {error}"), true)
            } else {
                (BloatwareStatus::Pending, "Disable only this optional ad, popup or statistics component. GoogleSignInPopupService and BlueStacksTokenService remain enabled.".into(), true)
            },
        };
        BloatwareItem { package: entry.target.into(), name: entry.name.into(), status, reason, can_clean }
    }).collect()
}
fn inventory(
    installed: &HashSet<String>,
    disabled: &HashSet<String>,
    home: Option<&str>,
    errors: &BTreeMap<String, String>,
    names: &BTreeMap<String, String>,
) -> Vec<BloatwareItem> {
    let candidates: HashSet<String> = installed
        .iter()
        .filter(|p| vendor(p))
        .cloned()
        .chain(ALLOWED.iter().map(|p| p.to_string()))
        .collect();
    let mut items = Vec::new();
    for package in candidates {
        let (status, reason, can_clean) = if !installed.contains(&package) {
            (
                BloatwareStatus::Cleared,
                "Not installed for Android user 0.".into(),
                false,
            )
        } else if disabled.contains(&package) {
            (
                BloatwareStatus::Cleared,
                "Already disabled for Android user 0. App data is retained.".into(),
                false,
            )
        } else if home == Some(package.as_str()) {
            (
                BloatwareStatus::Blocked,
                "This is the current Home launcher. Select another Home app before disabling it."
                    .into(),
                false,
            )
        } else if package == "com.bluestacks.BstCommandProcessor" {
            (BloatwareStatus::Blocked, "Core host integration: system-UID command service and accessibility bridge. Kept for emulator controls and app integration; not classified as bloatware.".into(), false)
        } else if package == "com.bluestacks.home" {
            (BloatwareStatus::Blocked, "Shared BS-Services package contains GoogleSignInPopupService and BlueStacksTokenService. Clean its individual ad components listed below instead of disabling this package.".into(), false)
        } else if !ALLOWED.contains(&package.as_str()) {
            (BloatwareStatus::Blocked,"Unrecognized vendor package. Its role must be identified before adding a cleanup operation for this build.".into(),false)
        } else if home.is_none()
            && (package.ends_with("launcher") || package == "com.uncube.launcher3")
        {
            (BloatwareStatus::Blocked,"The current Home launcher could not be verified. Retry after Android has finished booting.".into(),false)
        } else if let Some(error) = errors.get(&package) {
            (
                BloatwareStatus::Failed,
                format!("Last attempt failed: {error}"),
                true,
            )
        } else {
            (BloatwareStatus::Pending, effect(&package).into(), true)
        };
        let name = names
            .get(&package)
            .filter(|s| !s.is_empty())
            .cloned()
            .unwrap_or_else(|| package.clone());
        items.push(BloatwareItem {
            package,
            name,
            status,
            reason,
            can_clean,
        });
    }
    items.sort_by(|a, b| a.package.cmp(&b.package));
    items
}
pub fn load_bloatware(engine: &Engine) -> Result<CleanupPlan> {
    let (installed, disabled, home) = query(engine)?;
    let mut errors = failures(engine);
    errors
        .messages
        .retain(|target, _| installed.contains(owner(target)) && !disabled.contains(owner(target)));
    let names = engine
        .scan()?
        .apps
        .into_iter()
        .map(|app| (app.package, app.name))
        .collect();
    let mut items = inventory(
        &installed,
        &disabled,
        home.as_deref(),
        &errors.messages,
        &names,
    );
    items.extend(component_inventory(
        &component_states(engine, &installed, &disabled)?,
        &errors.messages,
        !matches!(privilege(engine), Privilege::Unavailable),
    ));
    for item in &items {
        if item.status == BloatwareStatus::Cleared {
            errors.messages.remove(&item.package);
        }
    }
    storage::save(&engine.root.join("state/bloatware-failures.json"), &errors)?;
    let plan = CleanupPlan {
        id: uuid::Uuid::new_v4().simple().to_string(),
        changes: items
            .iter()
            .filter(|item| item.can_clean)
            .map(|item| crate::backend::CleanupChange {
                package: item.package.clone(),
                operation: operation(&item.package).into(),
            })
            .collect(),
        items,
        data_path: engine.installation.data_path.to_string_lossy().into(),
    };
    storage::save(
        &engine.root.join(format!("state/plan-{}.json", plan.id)),
        &plan,
    )?;
    Ok(plan)
}
pub fn apply(engine: &Engine, id: &str, selected: Option<&str>) -> Result<usize> {
    engine.require_android()?;
    if id.len() != 32 || !id.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("Load bloatware first.")
    }
    let plan: CleanupPlan = storage::read(&engine.root.join(format!("state/plan-{id}.json")))?;
    if !plan
        .data_path
        .eq_ignore_ascii_case(&engine.installation.data_path.to_string_lossy())
    {
        bail!("The installation changed. Load bloatware again before cleaning.")
    }
    for change in &plan.changes {
        if !allowed(&change.package) || change.operation != operation(&change.package) {
            bail!("Cleanup permits only identified optional packages and ad components.")
        }
    }
    let (installed, disabled, home) = query(engine)?;
    let mut errors = failures(engine);
    let mut current = inventory(
        &installed,
        &disabled,
        home.as_deref(),
        &errors.messages,
        &BTreeMap::new(),
    );
    current.extend(component_inventory(
        &component_states(engine, &installed, &disabled)?,
        &errors.messages,
        !matches!(privilege(engine), Privilege::Unavailable),
    ));
    let requested = if let Some(package) = selected {
        if !plan.changes.iter().any(|c| c.package == package) {
            bail!("Load bloatware again before cleaning this package.")
        }
        vec![package.to_owned()]
    } else {
        plan.items
            .iter()
            .filter(|item| item.status == BloatwareStatus::Pending)
            .map(|item| item.package.clone())
            .collect()
    };
    let mut targets = Vec::new();
    for package in requested {
        let item = current
            .iter()
            .find(|i| i.package == package)
            .ok_or_else(|| anyhow::anyhow!("Package is no longer listed: {package}"))?;
        if item.status == BloatwareStatus::Cleared {
            continue;
        }
        if !item.can_clean {
            bail!("{}: {}", item.package, item.reason)
        }
        targets.push(package);
    }
    if targets.is_empty() {
        return Ok(0);
    }
    let path = storage::local().join(format!("backups/{id}.json"));
    if path.exists() {
        bail!("This list has already been used. Load bloatware again.")
    }
    let mut backup = Backup {
        id: id.into(),
        instance: "Tiramisu64".into(),
        changes: vec![],
        completed: false,
        undone: false,
        data_path: engine.installation.data_path.to_string_lossy().into(),
    };
    storage::save(&path, &backup)?;
    let attempted = std::cell::RefCell::new(None::<String>);
    let result = transaction(
        &mut backup,
        &targets,
        |package| {
            *attempted.borrow_mut() = Some(package.to_owned());
            clean_target(engine, package)
        },
        |package| enable(engine, package),
        |record| storage::save(&path, record),
    );
    if let Err(error) = result {
        if let Some(package) = attempted.into_inner() {
            errors.messages.insert(package, format!("{error:#}"));
            storage::save(&engine.root.join("state/bloatware-failures.json"), &errors)?;
        }
        return Err(error);
    }
    for package in &targets {
        errors.messages.remove(package);
    }
    storage::save(&engine.root.join("state/bloatware-failures.json"), &errors)?;
    Ok(backup.changes.len())
}
pub fn undo(engine: &Engine) -> Result<usize> {
    engine.require_android()?;
    let base = storage::local().join("backups");
    let mut files = std::fs::read_dir(base)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .collect::<Vec<_>>();
    files.sort_by_key(|f| std::cmp::Reverse(f.metadata().and_then(|m| m.modified()).ok()));
    let path = files
        .first()
        .ok_or_else(|| anyhow::anyhow!("No cleanup backup is available."))?
        .path();
    let mut backup: Backup = storage::read(&path)?;
    validate_backup(&backup, &path)?;
    if !backup.data_path.is_empty()
        && !backup
            .data_path
            .eq_ignore_ascii_case(&engine.installation.data_path.to_string_lossy())
    {
        bail!("This cleanup backup belongs to a different Android data folder.")
    }
    if backup.undone {
        bail!("The last cleanup has already been undone.")
    }
    for package in &backup.changes {
        enable(engine, package)?;
    }
    backup.undone = true;
    storage::save(&path, &backup)?;
    Ok(backup.changes.len())
}
fn enable(engine: &Engine, target: &str) -> Result<()> {
    if !target.contains('/') {
        let installed =
            packages(&engine.shell(&["pm", "list", "packages", "--user", "0", target], 10)?);
        if !installed.contains(target) {
            engine.shell(
                &["cmd", "package", "install-existing", "--user", "0", target],
                20,
            )?;
        }
    }
    let args = ["pm", "enable", "--user", "0", target];
    let reply = if target.contains('/') {
        privileged_shell(engine, &args)?
    } else {
        engine.shell(&args, 15)?
    };
    if !reply.contains("new state: enabled") {
        bail!("Android refused to restore {target}: {}", reply.trim())
    }
    Ok(())
}
fn clean_target(engine: &Engine, target: &str) -> Result<()> {
    if target.contains('/') {
        let reply = privileged_shell(engine, &["pm", "disable-user", "--user", "0", target])?;
        if !reply.contains("new state: disabled-user") {
            bail!("Android refused component cleanup: {}", reply.trim())
        }
        return Ok(());
    }
    let attempt = engine.shell(&["pm", "disable-user", "--user", "0", target], 15);
    let installed =
        packages(&engine.shell(&["pm", "list", "packages", "--user", "0", target], 10)?);
    let disabled =
        packages(&engine.shell(&["pm", "list", "packages", "-d", "--user", "0", target], 10)?);
    if !installed.contains(target) || disabled.contains(target) {
        return Ok(());
    }
    let system =
        packages(&engine.shell(&["pm", "list", "packages", "-s", "--user", "0", target], 10)?);
    if !system.contains(target) {
        bail!(
            "Android did not disable {target}: {}. Per-user removal is only used for system apps with a retained base APK for Undo.",
            attempt.unwrap_or_else(|error| format!("{error:#}"))
        )
    }
    // BlueStacks can ignore disable-user. Retain the data and system APK,
    // remove only user 0's installation, and check the actual installed state.
    let reply = engine.shell(&["pm", "uninstall", "-k", "--user", "0", target], 20)?;
    let remaining =
        packages(&engine.shell(&["pm", "list", "packages", "--user", "0", target], 10)?);
    if remaining.contains(target) {
        bail!(
            "Android did not remove {target} for user 0: {}",
            reply.trim()
        )
    }
    Ok(())
}
fn transaction(
    record: &mut Backup,
    targets: &[String],
    mut disable: impl FnMut(&str) -> Result<()>,
    mut enable: impl FnMut(&str) -> Result<()>,
    mut persist: impl FnMut(&Backup) -> Result<()>,
) -> Result<()> {
    let result = (|| -> Result<()> {
        for package in targets {
            record.changes.push(package.clone());
            persist(record)?;
            disable(package)?;
        }
        record.completed = true;
        persist(record)
    })();
    if let Err(error) = result {
        let mut failures = Vec::new();
        for package in &record.changes {
            if let Err(e) = enable(package) {
                failures.push(format!("{package}: {e}"));
            }
        }
        record.completed = false;
        let _ = persist(record);
        if !failures.is_empty() {
            bail!("{error:#}. Restore failed for: {}", failures.join("; "))
        }
        return Err(error);
    }
    Ok(())
}
fn validate_backup(record: &Backup, _path: &Path) -> Result<()> {
    if record.instance != "Tiramisu64" || record.changes.iter().any(|p| !allowed(p)) {
        bail!("Invalid cleanup backup.")
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inventory_reports_live_state_and_excludes_google_and_games() {
        let installed = [
            "com.bluestacks.quest",
            "com.bluestacks.piggy",
            "com.bluestacks.settings",
            "com.google.android.gms",
            "com.android.vending",
            "com.example.game",
        ]
        .map(String::from)
        .into();
        let disabled = ["com.bluestacks.piggy".into()].into();
        let rows = inventory(
            &installed,
            &disabled,
            Some("org.fossify.home"),
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        let quest = rows
            .iter()
            .find(|row| row.package == "com.bluestacks.quest")
            .unwrap();
        assert_eq!(quest.status, BloatwareStatus::Pending);
        assert!(quest.can_clean);
        let piggy = rows
            .iter()
            .find(|row| row.package == "com.bluestacks.piggy")
            .unwrap();
        assert_eq!(piggy.status, BloatwareStatus::Cleared);
        assert!(piggy.reason.contains("disabled"));
        let absent = rows
            .iter()
            .find(|row| row.package == "com.bluestacks.nowgg")
            .unwrap();
        assert_eq!(absent.status, BloatwareStatus::Cleared);
        assert!(absent.reason.contains("Not installed"));
        let vendor = rows
            .iter()
            .find(|row| row.package == "com.bluestacks.settings")
            .unwrap();
        assert_eq!(vendor.status, BloatwareStatus::Pending);
        assert!(vendor.can_clean);
        assert!(!vendor.reason.is_empty());
        assert!(
            !rows.iter().any(|row| row.package.starts_with("com.google.")
                || row.package == "com.android.vending"
                || row.package == "com.example.game")
        );
    }
    #[test]
    fn home_launcher_is_protected_and_disabled_vendor_is_cleared() {
        let installed = ["com.bluestacks.bsxlauncher", "com.bluestacks.settings"]
            .map(String::from)
            .into();
        for home in [Some("com.bluestacks.bsxlauncher"), None] {
            let rows = inventory(
                &installed,
                &["com.bluestacks.settings".into()].into(),
                home,
                &BTreeMap::new(),
                &BTreeMap::new(),
            );
            let launcher = rows
                .iter()
                .find(|row| row.package == "com.bluestacks.bsxlauncher")
                .unwrap();
            assert_eq!(launcher.status, BloatwareStatus::Blocked);
            assert!(!launcher.can_clean);
            assert!(launcher.reason.contains("Home"));
            assert_eq!(
                rows.iter()
                    .find(|row| row.package == "com.bluestacks.settings")
                    .unwrap()
                    .status,
                BloatwareStatus::Cleared
            );
        }
    }
    #[test]
    fn failed_attempt_is_retryable_until_live_state_is_cleared() {
        let installed = ["com.bluestacks.quest".into()].into();
        let errors = [("com.bluestacks.quest".into(), "Permission denied".into())].into();
        let rows = inventory(
            &installed,
            &HashSet::new(),
            Some("org.fossify.home"),
            &errors,
            &BTreeMap::new(),
        );
        let quest = rows
            .iter()
            .find(|row| row.package == "com.bluestacks.quest")
            .unwrap();
        assert_eq!(quest.status, BloatwareStatus::Failed);
        assert!(quest.reason.contains("Permission denied"));
        assert!(quest.can_clean);
        let rows = inventory(
            &installed,
            &installed,
            Some("org.fossify.home"),
            &errors,
            &BTreeMap::new(),
        );
        let quest = rows
            .iter()
            .find(|row| row.package == "com.bluestacks.quest")
            .unwrap();
        assert_eq!(quest.status, BloatwareStatus::Cleared);
        assert!(!quest.can_clean);
    }
    #[test]
    fn identified_optional_packages_work_and_core_services_stay_protected() {
        let installed = ALLOWED
            .iter()
            .copied()
            .chain(["com.bluestacks.home", "com.bluestacks.BstCommandProcessor"])
            .map(String::from)
            .collect();
        let rows = inventory(
            &installed,
            &HashSet::new(),
            Some("org.fossify.home"),
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        for package in ALLOWED {
            let row = rows.iter().find(|row| row.package == *package).unwrap();
            assert_eq!(row.status, BloatwareStatus::Pending, "{package}");
            assert!(row.can_clean);
        }
        for package in ["com.bluestacks.home", "com.bluestacks.BstCommandProcessor"] {
            assert!(!allowed(package));
            let row = rows.iter().find(|row| row.package == package).unwrap();
            assert_eq!(row.status, BloatwareStatus::Blocked);
            assert!(!row.can_clean);
        }
    }
    #[test]
    fn component_states_and_errors_are_separate_from_parent_sign_in_services() {
        let target = AD_COMPONENTS[0].target;
        let states = [(target.into(), ComponentState::Enabled)].into();
        let errors = [(target.into(), "Android refused this operation".into())].into();
        let rows = component_inventory(&states, &errors, true);
        assert_eq!(rows[0].status, BloatwareStatus::Failed);
        assert!(rows[0].can_clean);
        assert!(rows[0].reason.contains("Android refused"));
        assert_eq!(rows[1].status, BloatwareStatus::Cleared);
        let rows = component_inventory(
            &[(target.into(), ComponentState::Disabled)].into(),
            &errors,
            true,
        );
        assert_eq!(rows[0].status, BloatwareStatus::Cleared);
        assert!(!rows[0].can_clean);
        assert!(allowed(target));
        assert!(!allowed(
            "com.bluestacks.home/com.bluestacks.svc.services.GoogleSignInPopupService"
        ));
        assert!(!allowed(
            "com.bluestacks.home/com.bluestacks.svc.services.BlueStacksTokenService"
        ));
        let rows = component_inventory(&states, &BTreeMap::new(), false);
        assert_eq!(rows[0].status, BloatwareStatus::Blocked);
        assert!(!rows[0].can_clean);
        assert!(rows[0].reason.contains("root access"));
    }
    #[test]
    fn component_query_normalizes_short_names_and_rejects_other_components() {
        let target = "com.bluestacks.home/com.bluestacks.home.AffiliateWebViewActivity";
        assert!(component_reply_contains(
            "com.bluestacks.home/.AffiliateWebViewActivity\n",
            target
        ));
        assert!(component_reply_contains(target, target));
        assert!(!component_reply_contains("No activities found", target));
        assert!(!component_reply_contains(
            "com.bluestacks.home/.GoogleSignInPopupService",
            target
        ));
    }
    #[test]
    fn old_backups_remain_readable() {
        let backup:Backup=serde_json::from_str(r#"{"id":"x","instance":"Tiramisu64","changes":[{"package":"com.bluestacks.quest","operation":"disable-user","wasDisabled":false}],"completed":true}"#).unwrap();
        assert_eq!(backup.changes, vec!["com.bluestacks.quest"]);
        assert!(!backup.undone);
    }
    #[test]
    fn partial_failure_is_journaled_then_rolled_back() {
        use std::cell::RefCell;
        let state = RefCell::new(HashSet::<String>::new());
        let journal = RefCell::new(Vec::new());
        let mut backup = Backup {
            id: "x".into(),
            instance: "Tiramisu64".into(),
            changes: vec![],
            completed: false,
            undone: false,
            data_path: String::new(),
        };
        let result = transaction(
            &mut backup,
            &["first".into(), "second".into()],
            |p| {
                assert!(
                    journal
                        .borrow()
                        .last()
                        .is_some_and(|v: &Vec<String>| v.contains(&p.to_owned()))
                );
                state.borrow_mut().insert(p.into());
                if p == "second" {
                    bail!("Injected failure after Android changed state")
                }
                Ok(())
            },
            |p| {
                state.borrow_mut().remove(p);
                Ok(())
            },
            |record| {
                journal.borrow_mut().push(record.changes.clone());
                Ok(())
            },
        );
        assert!(result.is_err());
        assert!(state.borrow().is_empty());
        assert!(!backup.completed);
    }
    #[test]
    fn google_components_are_protected() {
        let all = [
            "com.bluestacks.quest",
            "com.google.android.gms",
            "com.android.vending",
            "com.google.android.webview",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        assert_eq!(changes(&all, &HashSet::new()), vec!["com.bluestacks.quest"]);
        assert!(changes(&all, &["com.bluestacks.quest".into()].into()).is_empty());
        let bad = Backup {
            id: "x".into(),
            instance: "Tiramisu64".into(),
            changes: vec!["com.google.android.gms".into()],
            completed: true,
            undone: false,
            data_path: String::new(),
        };
        assert!(validate_backup(&bad, Path::new("")).is_err());
    }
}
