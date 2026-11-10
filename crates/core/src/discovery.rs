use crate::{platform, storage};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};
use windows_sys::Win32::System::Registry::*;
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Installation {
    pub program_path: PathBuf,
    pub data_path: PathBuf,
    pub source: String,
    pub found: bool,
    pub mode: String,
}
fn folder(path: &Path) -> String {
    path.to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .to_lowercase()
}
fn data_root(path: &Path) -> Option<PathBuf> {
    if path.as_os_str().is_empty() {
        return None;
    }
    if path.join("bluestacks.conf").is_file() {
        Some(path.to_path_buf())
    } else {
        path.parent()
            .filter(|p| p.join("bluestacks.conf").is_file())
            .map(Path::to_path_buf)
    }
}
fn value(key: HKEY, name: &str) -> String {
    unsafe {
        let mut size = 0;
        let mut kind = 0;
        let name = platform::wide(name);
        if RegQueryValueExW(
            key,
            name.as_ptr(),
            std::ptr::null(),
            &mut kind,
            std::ptr::null_mut(),
            &mut size,
        ) != 0
            || ![REG_SZ, REG_EXPAND_SZ].contains(&kind)
        {
            return String::new();
        }
        let mut bytes = vec![0u16; size as usize / 2 + 1];
        if RegQueryValueExW(
            key,
            name.as_ptr(),
            std::ptr::null(),
            &mut kind,
            bytes.as_mut_ptr() as _,
            &mut size,
        ) != 0
        {
            return String::new();
        }
        let text = String::from_utf16_lossy(
            &bytes[..bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len())],
        );
        expand(&text)
    }
}
fn expand(s: &str) -> String {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn ExpandEnvironmentStringsW(source: *const u16, target: *mut u16, size: u32) -> u32;
    }
    let source = platform::wide(s.trim().trim_matches('"'));
    unsafe {
        let n = ExpandEnvironmentStringsW(source.as_ptr(), std::ptr::null_mut(), 0);
        if n == 0 {
            return s.into();
        }
        let mut target = vec![0u16; n as usize];
        let written = ExpandEnvironmentStringsW(source.as_ptr(), target.as_mut_ptr(), n);
        if written == 0 || written > n {
            return s.into();
        }
        String::from_utf16_lossy(&target[..written.saturating_sub(1) as usize])
    }
}
fn registry() -> Vec<Installation> {
    unsafe {
        let mut result = Vec::new();
        for hive in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
            for view in [KEY_WOW64_64KEY, KEY_WOW64_32KEY] {
                let mut key = std::ptr::null_mut();
                if RegOpenKeyExW(
                    hive,
                    platform::wide("SOFTWARE\\BlueStacks_nxt").as_ptr(),
                    0,
                    KEY_READ | view,
                    &mut key,
                ) == 0
                {
                    let data = [value(key, "UserDefinedDir"), value(key, "DataDir")]
                        .iter()
                        .find_map(|v| data_root(Path::new(v)))
                        .unwrap_or_default();
                    result.push(Installation {
                        program_path: value(key, "InstallDir").into(),
                        data_path: data,
                        source: "Windows registry".into(),
                        ..Default::default()
                    });
                    RegCloseKey(key);
                }
                if RegOpenKeyExW(
                    hive,
                    platform::wide("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall")
                        .as_ptr(),
                    0,
                    KEY_READ | view,
                    &mut key,
                ) == 0
                {
                    let mut index = 0;
                    loop {
                        let mut name = [0u16; 512];
                        let mut size = 512;
                        if RegEnumKeyExW(
                            key,
                            index,
                            name.as_mut_ptr(),
                            &mut size,
                            std::ptr::null(),
                            std::ptr::null_mut(),
                            std::ptr::null_mut(),
                            std::ptr::null_mut(),
                        ) != 0
                        {
                            break;
                        }
                        index += 1;
                        let mut sub = std::ptr::null_mut();
                        if RegOpenKeyExW(key, name.as_ptr(), 0, KEY_READ | view, &mut sub) != 0 {
                            continue;
                        }
                        let display = value(sub, "DisplayName");
                        if display.starts_with("BlueStacks") && !display.starts_with("BlueStacks X")
                        {
                            let mut path = value(sub, "InstallLocation");
                            if path.is_empty() {
                                let icon = value(sub, "DisplayIcon");
                                let icon = icon.split(",").next().unwrap_or("").trim_matches('"');
                                path = Path::new(icon)
                                    .parent()
                                    .unwrap_or(Path::new(""))
                                    .to_string_lossy()
                                    .into_owned();
                            }
                            if !path.is_empty() {
                                result.push(Installation {
                                    program_path: path.into(),
                                    source: "Windows uninstall registry".into(),
                                    ..Default::default()
                                });
                            }
                        }
                        RegCloseKey(sub);
                    }
                    RegCloseKey(key);
                }
            }
        }
        result
    }
}
pub fn candidates() -> Vec<Installation> {
    let mut result = registry();
    for (_, path) in platform::processes() {
        if path
            .file_name()
            .is_some_and(|n| n.eq_ignore_ascii_case("HD-Player.exe"))
        {
            result.push(Installation {
                program_path: path.parent().unwrap_or(Path::new("")).into(),
                source: "Running BlueStacks process".into(),
                ..Default::default()
            });
        }
    }
    for name in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(base) = std::env::var_os(name) {
            result.push(Installation {
                program_path: PathBuf::from(base).join("BlueStacks_nxt"),
                source: "Standard installation folder".into(),
                ..Default::default()
            });
        }
    }
    result
}
pub fn resolve(
    selection: Option<Installation>,
    candidates: &[Installation],
    default: &Path,
) -> Installation {
    let manual = selection.filter(|s| s.mode != "auto" && !s.program_path.as_os_str().is_empty());
    let choices = manual
        .as_ref()
        .map(std::slice::from_ref)
        .unwrap_or(candidates);
    for choice in choices {
        let path = &choice.program_path;
        if path.as_os_str().is_empty() {
            continue;
        }
        let found = path.join("HD-Player.exe").is_file() && path.join("HD-Adb.exe").is_file();
        if !found && manual.is_none() {
            continue;
        }
        let explicit = manual
            .as_ref()
            .filter(|s| !s.data_path.as_os_str().is_empty());
        let data = if let Some(selected) = explicit {
            data_root(&selected.data_path)
        } else {
            data_root(&choice.data_path)
                .or_else(|| {
                    candidates
                        .iter()
                        .filter(|c| folder(&c.program_path) == folder(path))
                        .find_map(|c| data_root(&c.data_path))
                })
                .or_else(|| data_root(path))
                .or_else(|| data_root(default))
        };
        return Installation {
            program_path: path.components().collect(),
            data_path: data.unwrap_or_default().components().collect(),
            source: if manual.is_some() {
                "Saved folder selection".into()
            } else {
                choice.source.clone()
            },
            found,
            mode: choice.mode.clone(),
        };
    }
    Installation {
        source: "Not detected".into(),
        ..Default::default()
    }
}
pub fn detect(root: &Path, automatic: bool) -> Installation {
    let selection = if automatic {
        None
    } else {
        storage::read(&root.join("install.json")).ok()
    };
    let fallback =
        PathBuf::from(std::env::var_os("ProgramData").unwrap_or_default()).join("BlueStacks_nxt");
    resolve(selection, &candidates(), &fallback)
}
pub fn select(root: &Path, program: Option<&Path>, data: Option<&Path>) -> Result<Installation> {
    let old = detect(root, false);
    let selected = program.map(|path| Installation {
        program_path: path.into(),
        data_path: data.unwrap_or(Path::new("")).into(),
        mode: "manual".into(),
        ..Default::default()
    });
    let fallback =
        PathBuf::from(std::env::var_os("ProgramData").unwrap_or_default()).join("BlueStacks_nxt");
    let mut new = resolve(selected, &candidates(), &fallback);
    if !new.found {
        bail!("Select the folder containing HD-Player.exe and HD-Adb.exe.")
    }
    if data.is_some() && new.data_path.as_os_str().is_empty() {
        bail!("Select the data folder containing bluestacks.conf.")
    }
    if (folder(&old.program_path) != folder(&new.program_path)
        || folder(&old.data_path) != folder(&new.data_path))
        && platform::player_running()
    {
        bail!("Close all emulator windows before changing installation folders.")
    }
    new.mode = if program.is_some() { "manual" } else { "auto" }.into();
    storage::save(&root.join("install.json"), &new)?;
    if folder(&old.data_path) != folder(&new.data_path) {
        let cache = root.join("state/apps-live.json");
        if cache.exists() {
            fs::rename(
                cache,
                root.join(format!(
                    "state/apps-live.previous-{}.json",
                    uuid::Uuid::new_v4()
                )),
            )?;
        }
    }
    Ok(new)
}
pub fn config(location: &Installation) -> Result<String> {
    Ok(
        fs::read_to_string(location.data_path.join("bluestacks.conf"))?
            .trim_start_matches('\u{feff}')
            .into(),
    )
}
pub fn setting(config: &str, key: &str) -> Option<String> {
    config
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{key}=\"")))
        .map(|value| value.trim_end_matches(['\r', '"']).into())
}
pub fn serial(location: &Installation) -> String {
    config(location)
        .ok()
        .and_then(|text| setting(&text, "bst.instance.Tiramisu64.adb_port"))
        .filter(|p| p.parse::<u16>().is_ok_and(|n| n > 0))
        .map_or("127.0.0.1:5555".into(), |p| format!("127.0.0.1:{p}"))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn custom_paths_and_stale_entries() {
        let temp = std::env::temp_dir().join(format!("rust-install-{}", uuid::Uuid::new_v4()));
        let program = temp.join("Other drive/Apps & games");
        let data = temp.join("Separate data");
        fs::create_dir_all(&program).unwrap();
        fs::create_dir_all(&data).unwrap();
        for name in ["HD-Player.exe", "HD-Adb.exe"] {
            fs::write(program.join(name), []).unwrap();
        }
        fs::write(data.join("bluestacks.conf"), []).unwrap();
        let items = [
            Installation {
                program_path: temp.join("stale"),
                ..Default::default()
            },
            Installation {
                program_path: program.clone(),
                data_path: data.clone(),
                source: "Windows registry".into(),
                ..Default::default()
            },
        ];
        let found = resolve(None, &items, Path::new(""));
        assert!(found.found);
        assert_eq!(found.data_path, data);
        let chosen = resolve(
            Some(Installation {
                program_path: temp.join("missing"),
                mode: "manual".into(),
                ..Default::default()
            }),
            &items,
            Path::new(""),
        );
        assert!(!chosen.found);
        assert!(!resolve(None, &[], Path::new("")).found);
    }
}
