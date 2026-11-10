use crate::{
    backend::*,
    cleanup,
    discovery::{self, Installation},
    platform, storage,
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
pub const LOCK: &str = "Local\\MinimalBlueStackGamesLauncher";
#[derive(Clone)]
pub struct Engine {
    pub root: PathBuf,
    pub installation: Installation,
    pub serial: String,
    pub title: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Game {
    pub id: String,
    pub name: String,
    pub package: String,
    pub activity: String,
    pub icon: String,
    #[serde(default)]
    pub preferred_slot: Option<usize>,
}
#[derive(Default, Serialize, Deserialize)]
pub struct Catalog {
    #[serde(default)]
    pub schema: u32,
    #[serde(default)]
    pub games: Vec<Game>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Assignment {
    pub slot: usize,
    pub game: String,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub player_pid: u32,
    #[serde(default)]
    pub slots: Vec<Assignment>,
}
#[derive(Serialize, Deserialize)]
struct Manifest {
    files: Vec<FileHash>,
}
#[derive(Serialize, Deserialize)]
struct FileHash {
    scope: String,
    name: String,
    sha256: String,
}
impl Engine {
    pub fn new(root: &Path) -> Self {
        let installation = discovery::detect(root, false);
        let serial = discovery::serial(&installation);
        let title = discovery::config(&installation)
            .ok()
            .and_then(|c| discovery::setting(&c, "bst.instance.Tiramisu64.display_name"))
            .unwrap_or("Android 13 - Split Screen".into());
        Self {
            root: root.into(),
            installation,
            serial,
            title,
        }
    }
    pub fn validate(&self) -> Result<()> {
        if !self.installation.found {
            bail!("BlueStacks was not found. Use Browse to select its installation folder.")
        }
        if self.installation.data_path.as_os_str().is_empty() {
            bail!("Select the data folder containing bluestacks.conf.")
        }
        let manifest: Manifest = storage::read(&self.root.join("launcher-manifest.json"))
            .context("Install the native core before connecting Android")?;
        for name in [
            "HD-Player.exe",
            "BstNativeWindows.dll",
            "libOpenglRender.dll",
        ] {
            if !manifest
                .files
                .iter()
                .any(|f| f.scope == "program" && f.name == name)
            {
                bail!("Core manifest is incomplete: {name}")
            }
        }
        for name in [
            "minimal-guest.jar",
            "libminimal_guest.so",
            "MinimalGameGuard.exe",
        ] {
            if !self.root.join(name).is_file() {
                bail!("Rust runtime component is missing: {name}. Run cargo xtask build.")
            }
        }
        for entry in manifest.files {
            if Path::new(&entry.name).components().count() != 1 {
                bail!("Invalid manifest entry")
            }
            let base = if entry.scope == "program" {
                &self.installation.program_path
            } else {
                &self.root
            };
            if storage::hash(&base.join(&entry.name))? != entry.sha256 {
                bail!(
                    "File version changed: {}. Check the core before continuing.",
                    entry.name
                )
            }
        }
        let text = discovery::config(&self.installation)?;
        if discovery::setting(&text, "bst.enable_adb_access").as_deref() != Some("1")
            || discovery::setting(&text, "bst.enable_adb_remote_access").as_deref() != Some("0")
        {
            bail!("Enable local ADB and disable remote ADB using core setup.")
        }
        Ok(())
    }
    pub fn adb(&self, args: &[&str], seconds: u64) -> Result<String> {
        if !self.installation.found {
            bail!("Select the BlueStacks installation folder first.")
        }
        if args.first() != Some(&"kill-server")
            && std::net::TcpStream::connect_timeout(
                &"127.0.0.1:5038".parse().unwrap(),
                Duration::from_millis(100),
            )
            .is_err()
        {
            // A daemon must never inherit the command runner's output pipes.
            platform::daemon(
                &self.installation.program_path.join("HD-Adb.exe"),
                &["-P", "5038", "start-server"],
                &self.root,
            )?;
        }
        let mut argv = vec!["-P".into(), "5038".into()];
        argv.extend(args.iter().map(|s| s.to_string()));
        platform::run(
            &self.installation.program_path.join("HD-Adb.exe"),
            &argv,
            &self.root,
            seconds,
        )
    }
    pub fn shell(&self, args: &[&str], seconds: u64) -> Result<String> {
        let command = args
            .iter()
            .map(|arg| format!("'{}'", arg.replace('\'', "'\\''")))
            .collect::<Vec<_>>()
            .join(" ");
        self.script(&command, seconds)
    }
    fn script(&self, command: &str, seconds: u64) -> Result<String> {
        self.adb(&["-s", &self.serial, "shell", command], seconds)
    }
    pub fn require_android(&self) -> Result<()> {
        let _ = self.adb(&["connect", &self.serial], 5);
        if self
            .shell(&["getprop", "ro.build.version.release"], 5)?
            .trim()
            != "13"
        {
            bail!("Connect the Android 13 instance first.")
        }
        Ok(())
    }
    pub fn catalog(&self) -> Result<Catalog> {
        let path = self.root.join("games.json");
        if !path.exists() {
            return Ok(Catalog {
                schema: 1,
                games: vec![],
            });
        }
        let catalog: Catalog = storage::read(&path)?;
        for game in &catalog.games {
            validate_game(game)?;
        }
        Ok(catalog)
    }
    pub fn game(&self, id: &str) -> Result<Game> {
        self.catalog()?
            .games
            .into_iter()
            .find(|g| g.id == id)
            .ok_or_else(|| anyhow::anyhow!("App is not registered: {id}"))
    }
    pub fn session(&self, pid: u32) -> Session {
        storage::read::<Session>(&self.root.join("session-games.json"))
            .ok()
            .filter(|s| s.player_pid == pid)
            .unwrap_or(Session {
                player_pid: pid,
                slots: vec![],
            })
    }
    pub fn save_session(&self, session: &Session) -> Result<()> {
        storage::save(&self.root.join("session-games.json"), session)?;
        let mut ini = format!("[session]\r\nPlayerPid={}\r\n", session.player_pid);
        for assignment in &session.slots {
            let game = self.game(&assignment.game)?;
            let icon = self.icon_path(&game)?;
            ini += &format!(
                "\r\n[slot{}]\r\nName={}\r\nIcon={}\r\n",
                assignment.slot,
                game.name,
                icon.display()
            );
        }
        let mut bytes = vec![255, 254];
        bytes.extend(ini.encode_utf16().flat_map(u16::to_le_bytes));
        fs::write(self.root.join("session-icons.ini"), bytes)?;
        Ok(())
    }
    pub fn icon_path(&self, game: &Game) -> Result<PathBuf> {
        let relative = Path::new(&game.icon);
        if relative.is_absolute()
            || relative
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
            || !relative.starts_with("icons")
        {
            bail!("App icon must be inside the runtime icons folder.")
        }
        let path = self.root.join(relative);
        if !path.is_file() {
            bail!("App icon is missing.")
        }
        Ok(path)
    }
    pub fn register(&self, app: &AndroidApp, shortcut: bool) -> Result<String> {
        validate_package(&app.package)?;
        if app.activity.is_empty() {
            bail!("This app has no launcher activity.")
        }
        let _lock = platform::MutexGuard::lock(LOCK)?;
        let mut catalog = self.catalog()?;
        let existing = catalog
            .games
            .iter()
            .find(|g| g.package == app.package)
            .cloned();
        let id = existing.as_ref().map(|g| g.id.clone()).unwrap_or_else(|| {
            format!("App_{:X}", Sha256::digest(app.package.as_bytes()))[..20].into()
        });
        let mut game = Game {
            id: id.clone(),
            name: existing
                .as_ref()
                .map_or_else(|| app.name.clone(), |g| g.name.clone()),
            package: app.package.clone(),
            activity: app.activity.clone(),
            icon: format!("icons/{id}.ico"),
            preferred_slot: existing.and_then(|g| g.preferred_slot),
        };
        if game.name.is_empty() {
            game.name = app.package.clone();
        }
        validate_game(&game)?;
        let icon = PathBuf::from(&app.icon);
        if !icon.is_file() {
            bail!("Scan Android to export the app's original icon.")
        }
        let bytes = fs::read(&icon)?;
        let bytes = wrap_icon(&bytes)?;
        fs::create_dir_all(self.root.join("icons"))?;
        fs::write(self.root.join(&game.icon), bytes)?;
        catalog.games.retain(|g| g.package != app.package);
        catalog.games.push(game.clone());
        storage::save(&self.root.join("games.json"), &catalog)?;
        if shortcut {
            let file = game
                .name
                .chars()
                .map(|c| {
                    if "<>:\"/\\|?*".contains(c) || c.is_control() {
                        '_'
                    } else {
                        c
                    }
                })
                .take(100)
                .collect::<String>();
            let destination =
                platform::desktop().join(format!("{}.lnk", file.trim().trim_end_matches('.')));
            platform::shortcut(
                &destination,
                &std::env::current_exe()?,
                &format!(
                    "--launch {} --root {}",
                    game.id,
                    platform::quote(&self.root.to_string_lossy())
                ),
                &self.root,
                &self.root.join(&game.icon),
                &platform::game_app_id(&self.root, &game.package),
            )?;
        }
        Ok(id)
    }
    pub fn status(&self) -> HostStatus {
        let result = self.validate();
        let pid = platform::main_player(&self.title);
        HostStatus {
            installed: self.installation.found,
            version: platform::version(&self.installation.program_path.join("HD-Player.exe")),
            program: self.installation.program_path.to_string_lossy().into(),
            data_path: self.installation.data_path.to_string_lossy().into(),
            detection_source: self.installation.source.clone(),
            instance: "Tiramisu64".into(),
            ready: result.is_ok(),
            problem: result.err().map(|e| format!("{e:#}")),
            running: pid != 0,
            player_pid: pid,
            max_windows: 2,
        }
    }
    pub fn scan(&self) -> Result<Snapshot> {
        let catalog = self.catalog()?;
        let live = self.root.join("state/apps-live.json");
        let source;
        if live.is_file() {
            source = "Android snapshot";
        } else {
            source = "BlueStacks cache";
        }
        let mut apps: Vec<AndroidApp> = if live.is_file() {
            storage::read(&live)?
        } else {
            let cache = self
                .installation
                .data_path
                .join("Engine/Tiramisu64/AppCache");
            let entries: Vec<Value> =
                storage::read(&cache.join("AppCache.json")).unwrap_or_default();
            entries
                .into_iter()
                .map(|row| {
                    let package = row["package"].as_str().unwrap_or("").to_owned();
                    let activity =
                        format!("{}/{}", package, row["activity"].as_str().unwrap_or(""));
                    AndroidApp {
                        package,
                        name: row["appLabel"].as_str().unwrap_or("").into(),
                        activity,
                        system: row["isHomeApp"].as_bool().unwrap_or(false),
                        enabled: true,
                        icon: cache
                            .join(row["iconFileName"].as_str().unwrap_or(""))
                            .to_string_lossy()
                            .into(),
                        ..Default::default()
                    }
                })
                .collect()
        };
        for app in &mut apps {
            if !app.icon.is_empty() && !Path::new(&app.icon).is_absolute() {
                app.icon = self
                    .root
                    .join("state/minimal-app-icons")
                    .join(Path::new(&app.icon).file_name().unwrap_or_default())
                    .to_string_lossy()
                    .into();
            }
            if let Some(game) = catalog.games.iter().find(|g| g.package == app.package) {
                app.registered = true;
                app.game_id = game.id.clone();
                app.name = game.name.clone();
            }
            app.promotional = cleanup::ALLOWED.contains(&app.package.as_str());
        }
        Ok(Snapshot {
            status: self.status(),
            apps,
            source: source.into(),
        })
    }
    fn ensure_watch(&self, pid: u32) -> Result<()> {
        let watched = self.root.join(format!("state/watch-{pid}.json"));
        if storage::read::<u32>(&watched).is_ok_and(platform::alive) {
            return Ok(());
        }
        let legacy = self.root.join("AndroidGameHostGuard.exe");
        if self.session(pid).slots.len() > 0
            && platform::processes()
                .iter()
                .any(|(_, path)| path == &legacy)
        {
            return Ok(());
        }
        let child = platform::hidden(self.root.join("MinimalGameGuard.exe"))
            .args([
                "--pid",
                &pid.to_string(),
                "--root",
                &self.root.to_string_lossy(),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        storage::save(&watched, &child.id())?;
        Ok(())
    }
    pub fn boot(&self) -> Result<u32> {
        self.validate()?;
        let _lock = platform::MutexGuard::lock(LOCK)?;
        self.boot_locked()
    }
    fn boot_locked(&self) -> Result<u32> {
        let mut pid = platform::main_player(&self.title);
        if pid == 0 {
            pid = platform::spawn_player(&self.installation.program_path.join("HD-Player.exe"))?;
        }
        self.ensure_watch(pid)?;
        let deadline = Instant::now() + Duration::from_secs(150);
        while Instant::now() < deadline {
            let found = platform::main_player(&self.title);
            if found != 0 {
                pid = found;
                platform::hide_main(pid, &self.title);
                self.ensure_watch(pid)?;
            }
            let _ = self.adb(&["connect", &self.serial], 4);
            if self
                .shell(&["getprop", "sys.boot_completed"], 5)
                .is_ok_and(|s| s.trim() == "1")
            {
                self.require_android()?;
                return Ok(pid);
            }
            std::thread::sleep(Duration::from_millis(800));
        }
        bail!("Android has not finished booting. Try again shortly.")
    }
    pub fn upload_guest(&self) -> Result<(String, String)> {
        let mut paths = Vec::new();
        for file in ["minimal-guest.jar", "libminimal_guest.so"] {
            let path = self.root.join(file);
            if !path.is_file() {
                bail!("Build the Rust Android guest first: {file} is missing.")
            }
            let digest = storage::hash(&path)?;
            let remote = if file.ends_with(".jar") {
                format!("/data/local/tmp/minimal-guest-{digest}.jar")
            } else {
                format!("/data/local/tmp/libminimal_guest-{digest}.so")
            };
            // Loaded code is immutable. A second game or rescan must not truncate
            // a library or DEX file already mapped by the active guest process.
            let present =
                self.script(&format!("if [ -f '{remote}' ]; then printf present; fi"), 5)?;
            if present != "present" {
                self.adb(
                    &["-s", &self.serial, "push", &path.to_string_lossy(), &remote],
                    30,
                )?;
            }
            paths.push(remote);
        }
        Ok((paths.remove(0), paths.remove(0)))
    }
    pub fn guest(&self, mode: &str, slot: Option<usize>) -> Result<String> {
        let args = slot.map_or(String::new(), |s| format!(" {s}"));
        let (jar, library) = self.upload_guest()?;
        self.script(
            &format!(
                "CLASSPATH={jar} app_process /system/bin MinimalBootstrap {library} {mode}{args}"
            ),
            45,
        )
    }
    pub fn scan_live(&self) -> Result<()> {
        self.require_android()?;
        let text = self.guest("--inspect", None)?;
        let start = text
            .find('[')
            .context("Guest app scan did not return JSON")?;
        let apps = serde_json::Deserializer::from_str(&text[start..])
            .into_iter::<Vec<AndroidApp>>()
            .next()
            .context("Incomplete guest scan")??;
        fs::create_dir_all(self.root.join("state"))?;
        self.adb(
            &[
                "-s",
                &self.serial,
                "pull",
                "/data/local/tmp/minimal-app-icons",
                &self.root.join("state").to_string_lossy(),
            ],
            45,
        )?;
        storage::save(&self.root.join("state/apps-live.json"), &apps)?;
        Ok(())
    }
    pub fn launch(&self, id: &str, background: bool) -> Result<()> {
        self.validate()?;
        let _lock = platform::MutexGuard::lock(LOCK)?;
        let game = self.game(id)?;
        let pid = self.boot_locked()?;
        let mut session = self.session(pid);
        let slot = if let Some(a) = session.slots.iter().find(|s| s.game == id) {
            a.slot
        } else {
            let order = if game.preferred_slot == Some(1) {
                [1, 0]
            } else {
                [0, 1]
            };
            let slot = order
                .into_iter()
                .find(|s| !platform::slot_open(pid, Some(*s)))
                .context("Two windows are active. Close one before opening another game.")?;
            for old in session.slots.iter().filter(|a| a.slot == slot) {
                let spec = self.game(&old.game)?;
                self.shell(&["am", "force-stop", &spec.package], 15)?;
                let _ = self.guest("--close-display", Some(slot));
            }
            session.slots.retain(|s| s.slot != slot);
            session.slots.push(Assignment {
                slot,
                game: id.into(),
            });
            slot
        };
        self.save_session(&session)?;
        let running = self
            .shell(&["pidof", "bst-native-games"], 5)
            .unwrap_or_default();
        let (jar, library) = self.upload_guest()?;
        if running.split_whitespace().all(|x| x.parse::<u32>().is_ok())
            && !running.trim().is_empty()
        {
            self.guest("--ensure-display", Some(slot))?;
        } else {
            self.script(&format!("CLASSPATH={jar} nohup app_process /system/bin --nice-name=bst-native-games MinimalBootstrap {library} {slot} >/data/local/tmp/bst-native-displays.log 2>&1 </dev/null &"),15)?;
        }
        let display = loop_display(self, slot)?;
        self.adb(
            &["-s", &self.serial, "forward", "tcp:57015", "tcp:9166"],
            15,
        )?;
        self.adb(
            &["-s", &self.serial, "forward", "tcp:57016", "tcp:9167"],
            15,
        )?;
        let task = parse_task(
            &self.shell(&["dumpsys", "activity", "activities"], 15)?,
            &game.package,
        );
        if let Some(task) = &task {
            if task.display != display && task.id == task.root {
                self.shell(
                    &[
                        "am",
                        "display",
                        "move-stack",
                        &task.root.to_string(),
                        &display.to_string(),
                    ],
                    25,
                )?;
            }
        }
        if task
            .as_ref()
            .is_none_or(|t| !t.visible || (t.display != display && t.id != t.root))
        {
            self.shell(
                &[
                    "am",
                    "start",
                    "--display",
                    &display.to_string(),
                    "-f",
                    "0x18000000",
                    "-n",
                    &game.activity,
                ],
                25,
            )?;
        }
        let mut placed = false;
        for _ in 0..30 {
            let dump = self.shell(&["dumpsys", "activity", "activities"], 15)?;
            if let Some(task) = parse_task(&dump, &game.package) {
                if task.display == display {
                    placed = true;
                    break;
                }
                if task.id == task.root {
                    self.shell(
                        &[
                            "am",
                            "display",
                            "move-stack",
                            &task.root.to_string(),
                            &display.to_string(),
                        ],
                        15,
                    )?;
                }
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        if !placed {
            bail!("The app did not reach its assigned display. Saved data is preserved.")
        }
        for _ in 0..60 {
            platform::hide_main(pid, &self.title);
            for window in platform::windows().into_iter().filter(|window| {
                window.pid == pid
                    && window.class == "BlueStacksNativeGameWindow"
                    && window.slot == Some(slot)
            }) {
                if let Err(error) = crate::watcher::set_game_identity(
                    self,
                    &game,
                    window.handle,
                    &self.icon_path(&game)?,
                ) {
                    storage::log(
                        &self.root,
                        "watcher-events.log",
                        &format!("Taskbar identity: {error:#}"),
                    );
                }
            }
            let found = if background {
                platform::windows().iter().any(|w| {
                    w.pid == pid && w.class == "BlueStacksNativeGameWindow" && w.slot == Some(slot)
                })
            } else {
                platform::show_game(pid, slot, &game.name)
            };
            if found {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        bail!("Game window did not appear. Saved data is preserved.")
    }
    pub fn reconcile_displays(&self, pid: u32) -> Result<()> {
        let Some(_lock) = platform::MutexGuard::try_lock(LOCK)? else {
            return Ok(());
        };
        if !platform::alive(pid) {
            return Ok(());
        }
        let session = self.session(pid);
        if session.slots.is_empty() {
            return Ok(());
        }
        let targets = session
            .slots
            .iter()
            .map(|slot| Ok((slot.slot, self.game(&slot.game)?.package)))
            .collect::<Result<Vec<_>>>()?;
        let display_log = self.shell(&["cat", "/data/local/tmp/bst-native-displays.log"], 5)?;
        let dump = self.shell(&["dumpsys", "activity", "activities"], 10)?;
        for relocation in display_relocations(&dump, &display_log, &targets) {
            self.shell(
                &[
                    "am",
                    "display",
                    "move-stack",
                    &relocation.root.to_string(),
                    &relocation.display.to_string(),
                ],
                15,
            )?;
            storage::log(
                &self.root,
                "window-placement.log",
                &format!(
                    "Returned {} task {} to display {} after an activity transition.",
                    relocation.package, relocation.root, relocation.display
                ),
            );
        }
        Ok(())
    }
    pub fn stop_idle(&self) -> Result<()> {
        let pid = platform::main_player(&self.title);
        if pid != 0 {
            self.close(pid, None, true)?;
        } else if !platform::player_running() {
            let _ = self.adb(&["kill-server"], 5);
        }
        Ok(())
    }
    pub fn prepare_update(&self) -> Result<()> {
        self.validate()?;
        let pid = platform::main_player(&self.title);
        if pid == 0 {
            return self.stop_idle();
        }
        platform::request_game_close(pid);
        for _ in 0..50 {
            if !platform::slot_open(pid, None) {
                return self.stop_idle();
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        bail!("A game window did not close. The running emulator was preserved.")
    }
    pub fn close(&self, pid: u32, slot: Option<usize>, mut last: bool) -> Result<()> {
        let _lock = platform::MutexGuard::lock(LOCK)?;
        self.validate()?;
        if !platform::alive(pid) {
            return Ok(());
        }
        if !platform::processes().iter().any(|(id, path)| {
            *id == pid && path == &self.installation.program_path.join("HD-Player.exe")
        }) {
            bail!("Unexpected player identity; close cancelled.")
        }
        if platform::slot_open(pid, slot) {
            return Ok(());
        }
        if last && platform::slot_open(pid, None) {
            last = false;
        }
        let mut session = self.session(pid);
        let closed = session
            .slots
            .iter()
            .filter(|s| slot.is_none() || slot == Some(s.slot))
            .cloned()
            .collect::<Vec<_>>();
        let responsive = self.require_android().is_ok();
        if responsive {
            for assignment in &closed {
                let game = self.game(&assignment.game)?;
                self.shell(&["am", "force-stop", &game.package], 15)?;
                if !last {
                    let _ = self.guest("--close-display", Some(assignment.slot));
                }
            }
            session
                .slots
                .retain(|s| !closed.iter().any(|c| c.game == s.game));
            self.save_session(&session)?;
        } else if !last {
            bail!("Android is not responding. Other games were left running.")
        }
        if !last {
            return Ok(());
        }
        if platform::slot_open(pid, None) {
            return Ok(());
        }
        if responsive {
            self.shell(&["sync"], 15)?;
        }
        let vm = self.installation.program_path.join("BstkVMMgr.exe");
        let state = platform::run(
            &vm,
            &[
                "showvminfo".into(),
                "Tiramisu64".into(),
                "--machinereadable".into(),
            ],
            &self.root,
            30,
        )?;
        if state.contains("VMState=\"running\"") {
            platform::run(
                &vm,
                &["controlvm".into(), "Tiramisu64".into(), "poweroff".into()],
                &self.root,
                30,
            )?;
        }
        let state = platform::run(
            &vm,
            &[
                "showvminfo".into(),
                "Tiramisu64".into(),
                "--machinereadable".into(),
            ],
            &self.root,
            30,
        )?;
        if !state.contains("VMState=\"poweroff\"") {
            bail!("The VM is not powered off; its player was preserved.")
        }
        platform::stop(pid)?;
        if !platform::player_running() {
            let _ = self.adb(&["kill-server"], 5);
        }
        storage::log(
            &self.root,
            "session-close.log",
            &format!("Last game closed: VM powered off and Player {pid} stopped."),
        );
        Ok(())
    }
}
fn loop_display(engine: &Engine, slot: usize) -> Result<u32> {
    let re = regex::Regex::new(&format!("NATIVE_DISPLAY_SLOT={slot} ID=(\\d+)"))?;
    for _ in 0..30 {
        let log = engine
            .shell(&["cat", "/data/local/tmp/bst-native-displays.log"], 5)
            .unwrap_or_default();
        if log.contains("NATIVE_BUFFER_BROKER_READY") {
            if let Some(found) = re.captures_iter(&log).last() {
                return Ok(found[1].parse()?);
            }
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    bail!("The guest display is not ready. See the Android log.")
}
#[derive(Debug)]
struct Task {
    id: u32,
    root: u32,
    display: u32,
    visible: bool,
}
fn parse_task(dump: &str, package: &str) -> Option<Task> {
    let re = regex::Regex::new(r"^\s+\* Task\{.*? #(\d+) .*?A=\d+:([^\s}]+)").ok()?;
    let root = regex::Regex::new(r"rootTaskId=(\d+)").ok()?;
    let mut display = 0;
    let mut stopped = None;
    for line in dump.lines() {
        if let Some(id) = line
            .strip_prefix("Display #")
            .and_then(|s| s.split_whitespace().next())
            .and_then(|s| s.parse().ok())
        {
            display = id;
        }
        if let Some(c) = re.captures(line) {
            if &c[2] == package {
                let id = c[1].parse().ok()?;
                let task = Task {
                    id,
                    root: root
                        .captures(line)
                        .and_then(|r| r[1].parse().ok())
                        .unwrap_or(id),
                    display,
                    visible: line.contains("visible=true"),
                };
                if task.visible {
                    return Some(task);
                }
                if stopped.is_none() {
                    stopped = Some(task);
                }
            }
        }
    }
    stopped
}
#[derive(Debug, PartialEq)]
struct Relocation {
    package: String,
    root: u32,
    display: u32,
}
fn display_relocations(
    dump: &str,
    display_log: &str,
    targets: &[(usize, String)],
) -> Vec<Relocation> {
    let mut moves = Vec::new();
    for (slot, package) in targets {
        let Ok(pattern) = regex::Regex::new(&format!("NATIVE_DISPLAY_SLOT={slot} ID=(\\d+)"))
        else {
            continue;
        };
        let Some(display) = pattern
            .captures_iter(display_log)
            .last()
            .and_then(|value| value[1].parse::<u32>().ok())
        else {
            continue;
        };
        if let Some(task) = parse_task(dump, package) {
            // Never move a shared parent task containing other applications.
            if task.visible && task.id == task.root && task.display != display {
                moves.push(Relocation {
                    package: package.clone(),
                    root: task.root,
                    display,
                });
            }
        }
    }
    moves
}
fn validate_game(game: &Game) -> Result<()> {
    validate_package(&game.package)?;
    if !regex::Regex::new(r"^[A-Za-z][A-Za-z0-9_-]{0,47}$")?.is_match(&game.id)
        || !regex::Regex::new(r"^[A-Za-z0-9_.$]+/[A-Za-z0-9_.$]+$")?.is_match(&game.activity)
        || game.name.contains(['\r', '\n'])
        || game.name.len() > 480
    {
        bail!("Invalid app registration.")
    }
    Ok(())
}
pub fn wrap_icon(data: &[u8]) -> Result<Vec<u8>> {
    if data.len() > 22 && data[..4] == [0, 0, 1, 0] && u16::from_le_bytes([data[4], data[5]]) > 0 {
        return Ok(data.to_vec());
    }
    if data.len() < 24 || &data[..8] != b"\x89PNG\r\n\x1a\n" {
        bail!("Icon must be a PNG or ICO.")
    }
    let w = u32::from_be_bytes(data[16..20].try_into()?);
    let h = u32::from_be_bytes(data[20..24].try_into()?);
    if !(1..=256).contains(&w) || !(1..=256).contains(&h) {
        bail!("PNG icon dimensions must be 1–256 px.")
    }
    let mut out = vec![0, 0, 1, 0, 1, 0, w as u8, h as u8, 0, 0, 1, 0, 32, 0];
    out.extend((data.len() as u32).to_le_bytes());
    out.extend(22u32.to_le_bytes());
    out.extend(data);
    Ok(out)
}
impl Backend {
    pub fn discover() -> Self {
        let mut candidates = Vec::new();
        let args = std::env::args().collect::<Vec<_>>();
        if let Some(i) = args.iter().position(|a| a == "--root") {
            if let Some(path) = args.get(i + 1) {
                candidates.push(PathBuf::from(path));
            }
        }
        if let Some(path) = std::env::var_os("MINIMAL_BLUESTACK_RUNTIME") {
            candidates.push(path.into());
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(parent) = exe.parent() {
                candidates.push(parent.join("runtime"));
                candidates.push(parent.into());
            }
        }
        if let Ok(dir) = std::env::current_dir() {
            candidates.push(dir.join("runtime"));
        }
        let root = candidates
            .iter()
            .find(|p| {
                p.join("games.json").is_file()
                    || p.join("games.example.json").is_file()
                    || p.join("minimal-guest.jar").is_file()
            })
            .cloned()
            .unwrap_or_else(|| {
                candidates
                    .first()
                    .cloned()
                    .unwrap_or_else(|| PathBuf::from("runtime"))
            });
        Self { root }
    }
    pub fn scan(&self) -> Result<Snapshot> {
        Engine::new(&self.root).scan()
    }
    pub fn execute(&self, job: Job) -> Result<Outcome> {
        let engine = Engine::new(&self.root);
        let mut plan = None;
        let message = match job {
            Job::Scan => "App list refreshed from local data.".into(),
            Job::Connect => {
                engine.boot()?;
                engine.scan_live()?;
                "Android connected. Installed apps and icons have been scanned.".into()
            }
            Job::Stop => {
                engine.stop_idle()?;
                "Idle Android sessions stopped. Open games are still running.".into()
            }
            Job::Register(app) => {
                engine.register(&app, true)?;
                format!(
                    "Created a shortcut for {} with its original icon.",
                    app.name
                )
            }
            Job::Launch(app) => {
                let id = engine.register(&app, false)?;
                engine.launch(&id, false)?;
                format!("Opened a window for {}.", app.name)
            }
            Job::LoadBloatware => {
                if engine.require_android().is_err() {
                    engine.boot()?;
                }
                let value = cleanup::load_bloatware(&engine)?;
                let text = format!(
                    "Loaded {} bloatware entries from Android. See each entry for its current status and reason.",
                    value.items.len()
                );
                plan = Some(value);
                text
            }
            action @ (Job::Apply(_) | Job::CleanPackage(_, _)) => {
                let result = match action {
                    Job::Apply(id) => cleanup::apply(&engine, &id, None),
                    Job::CleanPackage(id, package) => cleanup::apply(&engine, &id, Some(&package)),
                    _ => unreachable!(),
                };
                let refresh_error = if result.is_ok() {
                    engine.scan_live().err()
                } else {
                    None
                };
                plan = Some(cleanup::load_bloatware(&engine)?);
                let mut message = match result {
                    Ok(count) => format!(
                        "Cleanup complete: {count} entries cleaned. Packages may be disabled or removed for user 0 with data retained. Statuses refreshed from Android."
                    ),
                    Err(error) => {
                        format!("Cleanup failed: {error:#}. Statuses refreshed from Android.")
                    }
                };
                if let Some(error) = refresh_error {
                    message.push_str(&format!(
                        " App library refresh failed: {error:#}. Use Scan Android to retry."
                    ));
                }
                message
            }
            Job::Undo => {
                let result = cleanup::undo(&engine);
                let refresh_error = if result.is_ok() {
                    engine.scan_live().err()
                } else {
                    None
                };
                plan = Some(cleanup::load_bloatware(&engine)?);
                let mut message = match result {
                    Ok(count) => format!(
                        "Restored {count} packages / components from the last cleanup. Statuses refreshed from Android."
                    ),
                    Err(error) => {
                        format!("Undo failed: {error:#}. Statuses refreshed from Android.")
                    }
                };
                if let Some(error) = refresh_error {
                    message.push_str(&format!(
                        " App library refresh failed: {error:#}. Use Scan Android to retry."
                    ));
                }
                message
            }
            Job::SelectInstallation(path) => {
                discovery::select(&self.root, Some(&path), None)?;
                "Installation folder saved.".into()
            }
            Job::SelectData(path, program) => {
                discovery::select(&self.root, Some(Path::new(&program)), Some(&path))?;
                "Android data folder saved.".into()
            }
            Job::AutoDetect => {
                discovery::select(&self.root, None, None)?;
                "Installation detection refreshed from Windows.".into()
            }
            Job::SetupCore => {
                crate::setup::request(&self.root, false)?;
                "Core installed. Android is ready to connect.".into()
            }
            Job::RestoreCore => {
                crate::setup::request(&self.root, true)?;
                "Core files restored from backup.".into()
            }
        };
        Ok(Outcome {
            snapshot: Some(self.scan()?),
            plan,
            message,
        })
    }
    pub fn stop_idle_on_exit(&self) {
        let _ = platform::hidden(std::env::current_exe().unwrap_or_default())
            .args(["--stop-idle", "--root", &self.root.to_string_lossy()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
    pub fn open_folder(&self) {
        let _ = Command::new("explorer.exe").arg(&self.root).spawn();
    }
    pub fn open_install_folder(&self, path: &str) {
        if Path::new(path).is_dir() {
            let _ = Command::new("explorer.exe").arg(path).spawn();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn startup_transition_returns_only_the_misplaced_game_to_its_display() {
        let dump = "Display #0 (activities from top to bottom):\n * Task{x #50 type=standard A=10090:com.zjcs.android.sea U=0 visible=true}\nDisplay #3 (activities from top to bottom):\n * Task{y #51 type=standard A=10091:com.netmarble.skiagb U=0 visible=true}";
        let displays =
            "NATIVE_DISPLAY_SLOT=0 ID=1\nNATIVE_DISPLAY_SLOT=0 ID=2\nNATIVE_DISPLAY_SLOT=1 ID=3";
        assert_eq!(
            display_relocations(
                dump,
                displays,
                &[
                    (0, "com.zjcs.android.sea".into()),
                    (1, "com.netmarble.skiagb".into())
                ]
            ),
            vec![Relocation {
                package: "com.zjcs.android.sea".into(),
                root: 50,
                display: 2
            }]
        );
    }
    #[test]
    fn placement_prefers_visible_tasks_and_never_moves_shared_parent_tasks() {
        let dump = "Display #0 (activities from top to bottom):\n * Task{x #20 type=standard A=10090:com.example.game U=0 visible=false}\nDisplay #2 (activities from top to bottom):\n * Task{y #21 type=standard A=10090:com.example.game U=0 visible=true}";
        assert_eq!(parse_task(dump, "com.example.game").unwrap().id, 21);
        assert!(
            display_relocations(
                dump,
                "NATIVE_DISPLAY_SLOT=0 ID=2",
                &[(0, "com.example.game".into())]
            )
            .is_empty()
        );
        let nested = "Display #0 (activities from top to bottom):\n * Task{x #20 type=standard A=10090:com.example.game U=0 rootTaskId=19 visible=true}";
        assert!(
            display_relocations(
                nested,
                "NATIVE_DISPLAY_SLOT=0 ID=2",
                &[(0, "com.example.game".into())]
            )
            .is_empty()
        );
        assert!(display_relocations(dump, "", &[(0, "com.example.game".into())]).is_empty());
    }
    #[test]
    fn task_parser_handles_stopped_and_nested_tasks() {
        let dump = "Display #3 (activities from top to bottom):\n * Task{x #20 type=standard A=10090:com.example.game U=0 rootTaskId=19 visible=false}";
        let t = parse_task(dump, "com.example.game").unwrap();
        assert_eq!((t.id, t.root, t.display, t.visible), (20, 19, 3, false));
        assert!(parse_task(dump, "com.other.app").is_none());
    }
    #[test]
    fn png_icon_conversion_preserves_artwork() {
        let mut png = b"\x89PNG\r\n\x1a\n00000000".to_vec();
        png.extend(256u32.to_be_bytes());
        png.extend(256u32.to_be_bytes());
        let icon = wrap_icon(&png).unwrap();
        assert_eq!(&icon[22..], &png);
        assert_eq!(wrap_icon(&icon).unwrap(), icon);
    }
}
