use anyhow::{Result, bail};
use std::{
    ffi::OsStr,
    io::Read,
    os::windows::{ffi::OsStrExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    System::{Diagnostics::ToolHelp::*, Threading::*},
    UI::WindowsAndMessaging::*,
};
const SYNCHRONIZE: u32 = 0x00100000;
pub fn wide(value: impl AsRef<OsStr>) -> Vec<u16> {
    value.as_ref().encode_wide().chain(Some(0)).collect()
}
pub fn show_error(message: &str) {
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            wide(message).as_ptr(),
            wide("Minimal BlueStack").as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}
pub fn hidden(exe: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(exe);
    command.creation_flags(CREATE_NO_WINDOW);
    command
}
pub fn run(exe: &Path, args: &[String], root: &Path, seconds: u64) -> Result<String> {
    let mut child = hidden(exe)
        .args(args)
        .env("ANDROID_USER_HOME", root.join("adb-home"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let out = child.stdout.take().unwrap();
    let err = child.stderr.take().unwrap();
    let reader = |mut pipe: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = pipe.read_to_end(&mut bytes);
            bytes
        })
    };
    let output = reader(Box::new(out));
    let errors = reader(Box::new(err));
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Command timed out: {}", exe.display());
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    let text = String::from_utf8_lossy(&output.join().unwrap_or_default()).into_owned()
        + &String::from_utf8_lossy(&errors.join().unwrap_or_default());
    if !status.success() {
        bail!(
            "{}: {}",
            exe.file_name().unwrap_or_default().to_string_lossy(),
            text.trim()
        )
    }
    Ok(text.trim().into())
}
pub fn daemon(exe: &Path, args: &[&str], root: &Path) -> Result<()> {
    let mut child = hidden(exe)
        .args(args)
        .env("ANDROID_USER_HOME", root.join("adb-home"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                bail!("Could not start the private ADB server.")
            }
            return Ok(());
        }
        if Instant::now() >= until {
            let _ = child.kill();
            let _ = child.wait();
            bail!("ADB server startup timed out.")
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}
pub struct ComScope(bool);
impl ComScope {
    pub fn new() -> Self {
        unsafe {
            Self(
                windows::Win32::System::Com::CoInitializeEx(
                    None,
                    windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
                )
                .is_ok(),
            )
        }
    }
}
impl Drop for ComScope {
    fn drop(&mut self) {
        if self.0 {
            unsafe {
                windows::Win32::System::Com::CoUninitialize();
            }
        }
    }
}
pub fn quote(value: &str) -> String {
    let mut out = String::from("\"");
    let mut slashes = 0;
    for c in value.chars() {
        if c == '\\' {
            slashes += 1;
            continue;
        }
        if c == '"' {
            out.extend(std::iter::repeat_n('\\', slashes * 2 + 1));
        } else {
            out.extend(std::iter::repeat_n('\\', slashes));
        }
        slashes = 0;
        out.push(c);
    }
    out.extend(std::iter::repeat_n('\\', slashes * 2));
    out.push('"');
    out
}
pub fn processes() -> Vec<(u32, PathBuf)> {
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return vec![];
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of_val(&entry) as u32;
        let mut result = Vec::new();
        let mut ok = Process32FirstW(snapshot, &mut entry);
        while ok != 0 {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, entry.th32ProcessID);
            if !process.is_null() {
                let mut buffer = vec![0u16; 32768];
                let mut n = buffer.len() as u32;
                if QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut n) != 0 {
                    result.push((
                        entry.th32ProcessID,
                        PathBuf::from(String::from_utf16_lossy(&buffer[..n as usize])),
                    ));
                }
                CloseHandle(process);
            }
            ok = Process32NextW(snapshot, &mut entry);
        }
        CloseHandle(snapshot);
        result
    }
}
pub fn player_running() -> bool {
    processes().iter().any(|(_, path)| {
        path.file_name()
            .is_some_and(|n| n.eq_ignore_ascii_case("HD-Player.exe"))
    })
}
pub fn alive(pid: u32) -> bool {
    unsafe {
        let h = OpenProcess(SYNCHRONIZE, 0, pid);
        if h.is_null() {
            return false;
        }
        let alive = WaitForSingleObject(h, 0) == WAIT_TIMEOUT;
        CloseHandle(h);
        alive
    }
}
pub fn stop(pid: u32) -> Result<()> {
    unsafe {
        let h = OpenProcess(PROCESS_TERMINATE | SYNCHRONIZE, 0, pid);
        if h.is_null() {
            return Ok(());
        }
        let ok = TerminateProcess(h, 0);
        WaitForSingleObject(h, 3000);
        CloseHandle(h);
        if ok == 0 {
            bail!("Could not stop process {pid}");
        }
        Ok(())
    }
}
pub fn spawn_player(exe: &Path) -> Result<u32> {
    unsafe {
        let mut startup: STARTUPINFOW = std::mem::zeroed();
        startup.cb = std::mem::size_of_val(&startup) as u32;
        startup.dwFlags = STARTF_USESHOWWINDOW | STARTF_USESTDHANDLES;
        startup.wShowWindow = SW_HIDE as u16;
        let mut process: PROCESS_INFORMATION = std::mem::zeroed();
        let mut command = wide(format!("\"{}\" --instance Tiramisu64", exe.display()));
        if CreateProcessW(
            std::ptr::null(),
            command.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            CREATE_NO_WINDOW,
            std::ptr::null(),
            std::ptr::null(),
            &startup,
            &mut process,
        ) == 0
        {
            bail!(
                "Could not start BlueStacks: {}",
                std::io::Error::last_os_error()
            )
        }
        let pid = process.dwProcessId;
        CloseHandle(process.hThread);
        CloseHandle(process.hProcess);
        Ok(pid)
    }
}
#[derive(Clone)]
pub struct Window {
    pub handle: HWND,
    pub pid: u32,
    pub title: String,
    pub class: String,
    pub slot: Option<usize>,
    pub open: bool,
    pub generation: usize,
}
pub fn windows() -> Vec<Window> {
    unsafe {
        unsafe extern "system" fn enumerate(h: HWND, data: isize) -> i32 {
            unsafe {
                let mut pid = 0;
                GetWindowThreadProcessId(h, &mut pid);
                let mut text = [0u16; 512];
                let n = GetWindowTextW(h, text.as_mut_ptr(), 512);
                let title = String::from_utf16_lossy(&text[..n as usize]);
                let n = GetClassNameW(h, text.as_mut_ptr(), 512);
                let class = String::from_utf16_lossy(&text[..n as usize]);
                let stored = GetPropW(h, wide("BstNativeGameSlot").as_ptr()) as usize;
                let slot = if stored == 1 || stored == 2 {
                    Some(stored - 1)
                } else if title == "Sword x Staff" || title.ends_with("Display 1") {
                    Some(0)
                } else if title == "Seven Knights Idle Adventure" || title.ends_with("Display 2") {
                    Some(1)
                } else {
                    None
                };
                (&mut *(data as *mut Vec<Window>)).push(Window {
                    handle: h,
                    pid,
                    title,
                    class,
                    slot,
                    open: IsWindowVisible(h) != 0 || IsIconic(h) != 0,
                    generation: GetPropW(h, wide("MinimalOpenGeneration").as_ptr()) as usize,
                });
                1
            }
        }
        let mut list = Vec::new();
        EnumWindows(Some(enumerate), &mut list as *mut _ as isize);
        list
    }
}
pub fn main_player(title: &str) -> u32 {
    windows()
        .iter()
        .find(|w| w.title == title)
        .map_or(0, |w| w.pid)
}
pub fn slot_open(pid: u32, slot: Option<usize>) -> bool {
    windows().iter().any(|w| {
        w.pid == pid
            && w.class == "BlueStacksNativeGameWindow"
            && w.open
            && (slot.is_none() || slot == w.slot)
    })
}
pub fn request_game_close(pid: u32) {
    for window in windows() {
        if window.pid == pid && window.class == "BlueStacksNativeGameWindow" {
            unsafe {
                PostMessageW(window.handle, WM_CLOSE, 0, 0);
            }
        }
    }
}
pub fn hide_main(pid: u32, title: &str) {
    unsafe {
        for window in windows() {
            if window.pid != pid
                || window.title != title
                || window.class == "BlueStacksNativeGameWindow"
            {
                continue;
            }
            let h = window.handle;
            let style = GetWindowLongPtrW(h, GWL_EXSTYLE);
            let background = (style
                | WS_EX_LAYERED as isize
                | WS_EX_TRANSPARENT as isize
                | WS_EX_TOOLWINDOW as isize
                | WS_EX_NOACTIVATE as isize)
                & !(WS_EX_APPWINDOW as isize);
            if style != background {
                SetWindowLongPtrW(h, GWL_EXSTYLE, background);
                SetWindowPos(
                    h,
                    std::ptr::null_mut(),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                );
            }
            SetLayeredWindowAttributes(h, 0, 0, LWA_ALPHA);
            if !window.open || IsIconic(h) != 0 {
                ShowWindowAsync(h, SW_SHOWNOACTIVATE);
            }
        }
    }
}
pub fn show_game(pid: u32, slot: usize, name: &str) -> bool {
    unsafe {
        let mut area = RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut area as *mut _ as _, 0);
        let height = (area.bottom - area.top - 70).clamp(360, 960);
        let width = (height - 40) * 540 / 960 + 20;
        let x = area.left + ((area.right - area.left - 2 * width - 16) / 2).max(0);
        for window in windows() {
            if window.pid == pid
                && window.class == "BlueStacksNativeGameWindow"
                && window.slot == Some(slot)
            {
                SetPropW(
                    window.handle,
                    wide("BstNativeGameSlot").as_ptr(),
                    (slot + 1) as _,
                );
                SetWindowTextW(window.handle, wide(name).as_ptr());
                if !window.open {
                    SetPropW(
                        window.handle,
                        wide("MinimalOpenGeneration").as_ptr(),
                        window.generation.wrapping_add(1).max(1) as _,
                    );
                }
                ShowWindowAsync(window.handle, SW_RESTORE);
                SetWindowPos(
                    window.handle,
                    std::ptr::null_mut(),
                    x + slot as i32 * (width + 16),
                    area.top + 25,
                    width,
                    height,
                    SWP_SHOWWINDOW,
                );
                SetForegroundWindow(window.handle);
                return true;
            }
        }
        false
    }
}
pub struct MutexGuard(HANDLE);
impl MutexGuard {
    pub fn try_lock(name: &str) -> Result<Option<Self>> {
        unsafe {
            let handle = CreateMutexW(std::ptr::null(), 0, wide(name).as_ptr());
            if handle.is_null() {
                bail!("Cannot create session lock");
            }
            match WaitForSingleObject(handle, 0) {
                WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(Some(Self(handle))),
                WAIT_TIMEOUT => {
                    CloseHandle(handle);
                    Ok(None)
                }
                _ => {
                    CloseHandle(handle);
                    bail!("Cannot acquire session lock")
                }
            }
        }
    }
    pub fn lock(name: &str) -> Result<Self> {
        unsafe {
            let h = CreateMutexW(std::ptr::null(), 0, wide(name).as_ptr());
            if h.is_null() {
                bail!("Cannot create session lock");
            }
            let state = WaitForSingleObject(h, 180000);
            if state != WAIT_OBJECT_0 && state != WAIT_ABANDONED {
                CloseHandle(h);
                bail!("Another operation is still running");
            }
            Ok(Self(h))
        }
    }
    pub fn busy(name: &str) -> bool {
        unsafe {
            let h = OpenMutexW(SYNCHRONIZE | MUTEX_MODIFY_STATE, 0, wide(name).as_ptr());
            if h.is_null() {
                return false;
            }
            let state = WaitForSingleObject(h, 0);
            if state == WAIT_OBJECT_0 || state == WAIT_ABANDONED {
                ReleaseMutex(h);
            }
            CloseHandle(h);
            state == WAIT_TIMEOUT
        }
    }
}
impl Drop for MutexGuard {
    fn drop(&mut self) {
        unsafe {
            ReleaseMutex(self.0);
            CloseHandle(self.0);
        }
    }
}
pub fn version(path: &Path) -> String {
    #[link(name = "version")]
    unsafe extern "system" {
        fn GetFileVersionInfoSizeW(file: *const u16, handle: *mut u32) -> u32;
        fn GetFileVersionInfoW(
            file: *const u16,
            handle: u32,
            length: u32,
            data: *mut std::ffi::c_void,
        ) -> i32;
        fn VerQueryValueW(
            data: *const std::ffi::c_void,
            sub: *const u16,
            value: *mut *mut std::ffi::c_void,
            length: *mut u32,
        ) -> i32;
    }
    unsafe {
        let file = wide(path);
        let size = GetFileVersionInfoSizeW(file.as_ptr(), std::ptr::null_mut());
        if size == 0 {
            return String::new();
        }
        let mut bytes = vec![0; size as usize];
        if GetFileVersionInfoW(file.as_ptr(), 0, size, bytes.as_mut_ptr() as _) == 0 {
            return String::new();
        }
        let mut info = std::ptr::null_mut();
        let mut length = 0;
        if VerQueryValueW(
            bytes.as_ptr() as _,
            wide("\\").as_ptr(),
            &mut info,
            &mut length,
        ) == 0
            || length < 24
        {
            return String::new();
        }
        let words = std::slice::from_raw_parts(info as *const u32, 6);
        format!(
            "{}.{}.{}.{}",
            words[2] >> 16,
            words[2] & 65535,
            words[3] >> 16,
            words[3] & 65535
        )
    }
}
pub fn shortcut(
    destination: &Path,
    exe: &Path,
    args: &str,
    root: &Path,
    icon: &Path,
    app_id: &str,
) -> Result<()> {
    use windows::{
        Win32::{System::Com::*, UI::Shell::*},
        core::PCWSTR,
    };
    unsafe {
        let initialized = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
        let result = (|| -> Result<()> {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            if destination.exists() {
                let file: IPersistFile = windows::core::Interface::cast(&link)?;
                file.Load(PCWSTR(wide(destination).as_ptr()), STGM_READWRITE)?;
                let mut old_target = [0u16; 32768];
                let mut old_args = [0u16; 32768];
                link.GetPath(&mut old_target, std::ptr::null_mut(), 0)?;
                link.GetArguments(&mut old_args)?;
                let text = |bytes: &[u16]| {
                    String::from_utf16_lossy(
                        &bytes[..bytes.iter().position(|c| *c == 0).unwrap_or(bytes.len())],
                    )
                };
                let old_target = PathBuf::from(text(&old_target));
                let old_args = text(&old_args);
                let same =
                    old_target == exe && old_args.contains(&root.to_string_lossy().to_string());
                let legacy = old_target
                    .file_name()
                    .is_some_and(|n| n.eq_ignore_ascii_case("wscript.exe"))
                    && old_args.contains(
                        &root
                            .join("Open-AndroidGames.vbs")
                            .to_string_lossy()
                            .to_string(),
                    );
                if !same && !legacy {
                    bail!("An unrelated Desktop shortcut already uses this name.")
                }
            }
            link.SetPath(PCWSTR(wide(exe).as_ptr()))?;
            link.SetArguments(PCWSTR(wide(args).as_ptr()))?;
            link.SetWorkingDirectory(PCWSTR(wide(root).as_ptr()))?;
            link.SetIconLocation(PCWSTR(wide(icon).as_ptr()), 0)?;
            let properties = windows::core::Interface::cast(&link)?;
            set_shell_string(&properties, 5, app_id)?;
            properties.Commit()?;
            let file: IPersistFile = windows::core::Interface::cast(&link)?;
            file.Save(PCWSTR(wide(destination).as_ptr()), true)?;
            Ok(())
        })();
        if initialized {
            CoUninitialize();
        }
        result
    }
}
fn app_property(pid: u32) -> windows::Win32::Foundation::PROPERTYKEY {
    windows::Win32::Foundation::PROPERTYKEY {
        fmtid: windows::core::GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3),
        pid,
    }
}
fn set_shell_string(
    store: &windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore,
    pid: u32,
    value: &str,
) -> Result<()> {
    use windows::Win32::System::{Com::StructuredStorage::*, Variant::VT_LPWSTR};
    unsafe {
        let mut string = PROPVARIANT::default();
        PropVariantChangeType(
            &mut string,
            &PROPVARIANT::from(value),
            PVCHF_DEFAULT,
            VT_LPWSTR,
        )?;
        store.SetValue(&app_property(pid), &string)?;
    }
    Ok(())
}
pub fn game_app_id(root: &Path, package: &str) -> String {
    use sha2::{Digest, Sha256};
    let installation = root.to_string_lossy().to_lowercase();
    format!(
        "MinimalBlueStack.{:x}",
        Sha256::digest(format!("{installation}\0{package}").as_bytes())
    )
}
pub fn taskbar_identity(
    window: HWND,
    app_id: &str,
    name: &str,
    icon: &Path,
    command: &str,
) -> Result<()> {
    use windows::Win32::{System::Com::*, UI::Shell::PropertiesSystem::*};
    unsafe {
        let initialized = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
        let result = (|| -> Result<()> {
            let store: IPropertyStore =
                SHGetPropertyStoreForWindow(windows::Win32::Foundation::HWND(window))?;
            // Set relaunch metadata before the ID: assigning the ID notifies Explorer.
            set_shell_string(&store, 2, command)?;
            set_shell_string(&store, 3, &format!("{},0", icon.display()))?;
            set_shell_string(&store, 4, name)?;
            set_shell_string(&store, 5, app_id)?;
            Ok(())
        })();
        if initialized {
            CoUninitialize();
        }
        result
    }
}
pub fn desktop() -> PathBuf {
    use windows::Win32::UI::Shell::*;
    unsafe {
        let mut buffer = [0u16; 260];
        if SHGetFolderPathW(None, CSIDL_DESKTOPDIRECTORY as i32, None, 0, &mut buffer).is_ok() {
            return PathBuf::from(String::from_utf16_lossy(
                &buffer[..buffer.iter().position(|x| *x == 0).unwrap_or(0)],
            ));
        }
    }
    PathBuf::from(std::env::var_os("USERPROFILE").unwrap_or_default()).join("Desktop")
}
#[cfg(test)]
mod taskbar_tests {
    use super::*;
    use windows::Win32::{
        System::Com::*,
        UI::Shell::{PropertiesSystem::*, *},
    };
    #[test]
    fn windows_and_shortcuts_keep_their_own_game_identity() -> Result<()> {
        let root = std::env::temp_dir().join(format!("minimal-taskbar-{}", uuid::Uuid::new_v4()));
        let icon = root.with_extension("ico");
        let id = game_app_id(&root, "example.first");
        assert_ne!(id, game_app_id(&root, "example.second"));
        assert_ne!(
            id,
            game_app_id(&root.with_extension("other"), "example.first")
        );
        assert_eq!(
            id,
            game_app_id(
                &PathBuf::from(root.to_string_lossy().to_uppercase()),
                "example.first"
            )
        );
        assert!(id.len() <= 128);
        unsafe {
            let initialized = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
            let result = (|| -> Result<()> {
                let window = CreateWindowExW(
                    0,
                    wide("STATIC").as_ptr(),
                    wide("Taskbar identity test").as_ptr(),
                    0,
                    0,
                    0,
                    1,
                    1,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                );
                assert!(!window.is_null());
                let check = (|| -> Result<()> {
                    taskbar_identity(window, &id, "First game", &icon, "launcher --launch first")?;
                    let store: IPropertyStore =
                        SHGetPropertyStoreForWindow(windows::Win32::Foundation::HWND(window))?;
                    for (key, expected) in [
                        (5, id.clone()),
                        (3, format!("{},0", icon.display())),
                        (2, "launcher --launch first".into()),
                        (4, "First game".into()),
                    ] {
                        assert_eq!(store.GetValue(&app_property(key))?.to_string(), expected);
                    }
                    let second = game_app_id(&root, "example.second");
                    taskbar_identity(
                        window,
                        &second,
                        "Second game",
                        &icon,
                        "launcher --launch second",
                    )?;
                    assert_eq!(store.GetValue(&app_property(5))?.to_string(), second);
                    let destination = root.with_extension("lnk");
                    let saved = (|| -> Result<()> {
                        let args =
                            format!("--launch first --root {}", quote(&root.to_string_lossy()));
                        shortcut(
                            &destination,
                            &std::env::current_exe()?,
                            &args,
                            &root,
                            &icon,
                            &id,
                        )?;
                        // Existing shortcuts must remain writable when adding their AppID.
                        shortcut(
                            &destination,
                            &std::env::current_exe()?,
                            &args,
                            &root,
                            &icon,
                            &id,
                        )?;
                        let link: IShellLinkW =
                            CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
                        let file: IPersistFile = windows::core::Interface::cast(&link)?;
                        file.Load(
                            windows::core::PCWSTR(wide(&destination).as_ptr()),
                            STGM_READ,
                        )?;
                        let properties: IPropertyStore = windows::core::Interface::cast(&link)?;
                        assert_eq!(properties.GetValue(&app_property(5))?.to_string(), id);
                        Ok(())
                    })();
                    let _ = std::fs::remove_file(destination);
                    saved
                })();
                DestroyWindow(window);
                check
            })();
            if initialized {
                CoUninitialize();
            }
            result
        }
    }
}
