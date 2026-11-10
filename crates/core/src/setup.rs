use crate::{discovery, platform, storage};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{fs, path::Path};
pub const RENDERER: &str = "116bdb8cdbb9e980a462433d0db1a47bcb4b11946de5d8fe917de502ba330ee4";
const PLAYERS: &[&str] = &[
    "a560ac127282ec0a7467b764d9a910f86c686d6598049358b07b476d3a15b36c",
    "ec531f462e773fd1eed62158ac2bb999ff08465ff143b16f45409343bdb8b91b",
    "91da482193f2c4d7763314400ae2e59a373a0d8bb1692ae8b14e0963750d21a8",
];
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Receipt {
    program: String,
    before: String,
    after: String,
    bridge: String,
    backup_bridge_sha: Option<String>,
}
pub fn request(root: &Path, restore: bool) -> Result<()> {
    let _com = platform::ComScope::new();
    use windows_sys::Win32::{
        Foundation::*,
        System::Threading::*,
        UI::{Shell::*, WindowsAndMessaging::SW_HIDE},
    };
    unsafe {
        let exe = platform::wide(std::env::current_exe()?);
        let args = platform::wide(format!(
            "--install-core {} --root {}",
            if restore { "--restore" } else { "" },
            platform::quote(&root.to_string_lossy())
        ));
        let verb = platform::wide("runas");
        let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
        info.cbSize = std::mem::size_of_val(&info) as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
        info.lpVerb = verb.as_ptr();
        info.lpFile = exe.as_ptr();
        info.lpParameters = args.as_ptr();
        info.nShow = SW_HIDE;
        if ShellExecuteExW(&mut info) == 0 {
            bail!("Core setup was cancelled or could not be started.")
        }
        let status = WaitForSingleObject(info.hProcess, 180000);
        let mut code = 1;
        GetExitCodeProcess(info.hProcess, &mut code);
        CloseHandle(info.hProcess);
        if status == WAIT_TIMEOUT {
            bail!("Core setup is still pending. See state/core-install.json.")
        }
        if code != 0 {
            let result: serde_json::Value =
                storage::read(&root.join("state/core-install.json")).unwrap_or_default();
            bail!(
                "{}",
                result["error"].as_str().unwrap_or("Core setup failed.")
            )
        }
        Ok(())
    }
}
pub fn install(root: &Path, restore: bool) -> Result<()> {
    let result = install_inner(root, restore);
    let record = match &result {
        Ok(()) => json!({"ok":true}),
        Err(error) => json!({"ok":false,"error":format!("{error:#}")}),
    };
    let _ = storage::save(&root.join("state/core-install.json"), &record);
    result
}
fn install_inner(root: &Path, restore: bool) -> Result<()> {
    if platform::player_running() {
        bail!("Close all emulator windows before core setup or recovery.")
    }
    let location = discovery::detect(root, false);
    if !location.found {
        bail!("Select the BlueStacks installation folder first.")
    }
    let player = location.program_path.join("HD-Player.exe");
    let bridge = location.program_path.join("BstNativeWindows.dll");
    let renderer = location.program_path.join("libOpenglRender.dll");
    let program = location.program_path.to_string_lossy().into_owned();
    let mut backup = storage::local().join("core-backup");
    let legacy: Option<Receipt> = storage::read(&backup.join("receipt.json")).ok();
    if !legacy.as_ref().is_some_and(|r| {
        r.program
            .trim_end_matches('\\')
            .eq_ignore_ascii_case(&program)
    }) {
        let key = format!("{:X}", Sha256::digest(program.to_uppercase().as_bytes()));
        backup = backup.join(&key[..16]);
    }
    let receipt_path = backup.join("receipt.json");
    let previous: Option<Receipt> = storage::read(&receipt_path).ok();
    let metadata = [root.join("launcher-manifest.json"), receipt_path.clone()]
        .into_iter()
        .map(|path| {
            let bytes = fs::read(&path).ok();
            (path, bytes)
        })
        .collect::<Vec<_>>();
    let current = storage::hash(&player)?;
    let old_player = fs::read(&player)?;
    let old_bridge = fs::read(&bridge).ok();
    let config_path = location.data_path.join("bluestacks.conf");
    let old_config = fs::read(&config_path).ok();
    let result = (|| -> Result<()> {
        if restore {
            let record = previous
                .as_ref()
                .context("No core backup exists for this installation.")?;
            if current != record.after
                || storage::hash(&backup.join("HD-Player.before.exe"))? != record.before
            {
                bail!("Player or backup changed; recovery was cancelled.")
            }
            if bridge.exists() && storage::hash(&bridge)? != record.bridge {
                bail!("Bridge changed; recovery was cancelled.")
            }
            let original_bridge = backup.join("BstNativeWindows.before.dll");
            if original_bridge.exists()
                && record
                    .backup_bridge_sha
                    .as_ref()
                    .is_some_and(|h| storage::hash(&original_bridge).ok().as_ref() != Some(h))
            {
                bail!("Bridge backup checksum differs.")
            }
            fs::copy(backup.join("HD-Player.before.exe"), &player)?;
            if original_bridge.exists() {
                fs::copy(original_bridge, &bridge)?;
            } else if bridge.exists() {
                fs::remove_file(&bridge)?;
            }
            return Ok(());
        }
        if storage::hash(&renderer)? != RENDERER {
            bail!(
                "Unsupported renderer. This build requires BlueStacks 5.22.290.1031 / Android 13."
            )
        }
        if !PLAYERS.contains(&current.as_str())
            && !previous.as_ref().is_some_and(|r| r.after == current)
        {
            bail!("Unsupported player build. No vendor files were changed.")
        }
        let compiled = root.join("core/BstNativeWindows.dll");
        if !compiled.is_file() {
            bail!("Build the Rust native bridge first using cargo xtask build.")
        }
        let mut files = Vec::new();
        for name in [
            "minimal-guest.jar",
            "libminimal_guest.so",
            "MinimalGameGuard.exe",
        ] {
            files.push(
                json!({"scope":"bundle","name":name,"sha256":storage::hash(&root.join(name))?}),
            );
        }
        let text = String::from_utf8(
            old_config
                .clone()
                .context("Select the folder containing bluestacks.conf.")?,
        )?;
        let patched = patch_import(&old_player)?;
        fs::create_dir_all(&backup)?;
        if previous.is_none() {
            fs::write(backup.join("HD-Player.before.exe"), &old_player)?;
            if let Some(bytes) = &old_bridge {
                fs::write(backup.join("BstNativeWindows.before.dll"), bytes)?;
            }
            fs::write(backup.join("bluestacks.before.conf"), text.as_bytes())?;
        }
        fs::copy(&compiled, &bridge)?;
        fs::write(&player, patched)?;
        let mut config = text;
        for (key, value) in [
            ("bst.enable_adb_access", "1"),
            ("bst.enable_adb_remote_access", "0"),
        ] {
            let regex = regex::Regex::new(&format!("(?m)^{}=\"[^\"]*\"", regex::escape(key)))?;
            if !regex.is_match(&config) {
                bail!("Required ADB configuration entry is missing: {key}")
            }
            config = regex
                .replace_all(&config, format!("{key}=\"{value}\""))
                .into_owned();
        }
        fs::write(&config_path, config)?;
        for name in [
            "HD-Player.exe",
            "BstNativeWindows.dll",
            "libOpenglRender.dll",
        ] {
            files.push(json!({"scope":"program","name":name,"sha256":storage::hash(&location.program_path.join(name))?}));
        }
        let before = previous
            .as_ref()
            .map_or(current.clone(), |r| r.before.clone());
        let receipt = Receipt {
            program: program.clone(),
            before,
            after: storage::hash(&player)?,
            bridge: storage::hash(&bridge)?,
            backup_bridge_sha: storage::hash(&backup.join("BstNativeWindows.before.dll")).ok(),
        };
        storage::save(
            &root.join("launcher-manifest.json"),
            &json!({"instance":"Tiramisu64","adb_server_port":5038,"adb_serial":discovery::serial(&location),"files":files}),
        )?;
        storage::save(&receipt_path, &receipt)?;
        Ok(())
    })();
    if let Err(error) = result {
        let mut rollback = Vec::new();
        if fs::read(&player).ok().as_ref() != Some(&old_player) {
            if let Err(e) = fs::write(&player, &old_player) {
                rollback.push(e.to_string());
            }
        }
        match old_bridge {
            Some(bytes) => {
                if fs::read(&bridge).ok().as_ref() != Some(&bytes) {
                    if let Err(e) = fs::write(&bridge, bytes) {
                        rollback.push(e.to_string());
                    }
                }
            }
            None => {
                if bridge.exists() {
                    if let Err(e) = fs::remove_file(&bridge) {
                        rollback.push(e.to_string());
                    }
                }
            }
        }
        if let Some(bytes) = old_config {
            if fs::read(&config_path).ok().as_ref() != Some(&bytes) {
                if let Err(e) = fs::write(&config_path, bytes) {
                    rollback.push(e.to_string());
                }
            }
        }
        for (path, bytes) in metadata {
            let result = if let Some(bytes) = bytes {
                if fs::read(&path).ok().as_ref() == Some(&bytes) {
                    Ok(())
                } else {
                    fs::write(&path, bytes)
                }
            } else if path.exists() {
                fs::remove_file(&path)
            } else {
                Ok(())
            };
            if let Err(e) = result {
                rollback.push(e.to_string());
            }
        }
        if !rollback.is_empty() {
            bail!("{error:#}; rollback errors: {}", rollback.join("; "))
        }
        return Err(error);
    }
    Ok(())
}
fn u16_at(bytes: &[u8], p: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(
        bytes.get(p..p + 2).context("Truncated PE")?.try_into()?,
    ))
}
fn u32_at(bytes: &[u8], p: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        bytes.get(p..p + 4).context("Truncated PE")?.try_into()?,
    ))
}
fn put16(bytes: &mut [u8], p: usize, n: u16) {
    bytes[p..p + 2].copy_from_slice(&n.to_le_bytes());
}
fn put32(bytes: &mut [u8], p: usize, n: u32) {
    bytes[p..p + 4].copy_from_slice(&n.to_le_bytes());
}
fn align(n: usize, a: usize) -> Result<usize> {
    if a == 0 || !a.is_power_of_two() {
        bail!("Invalid PE alignment")
    }
    Ok(n.div_ceil(a) * a)
}
pub fn patch_import(bytes: &[u8]) -> Result<Vec<u8>> {
    let pe = u32_at(bytes, 0x3c)? as usize;
    if bytes.get(pe..pe + 4) != Some(b"PE\0\0") {
        bail!("Not a PE file")
    }
    let coff = pe + 4;
    let opt = pe + 24;
    if u16_at(bytes, opt)? != 0x20b {
        bail!("Expected PE64")
    }
    let count = u16_at(bytes, coff + 2)? as usize;
    let sections = opt + u16_at(bytes, coff + 16)? as usize;
    let dir = opt + 112;
    let mut entries = Vec::new();
    for i in 0..count {
        let p = sections + i * 40;
        entries.push((
            u32_at(bytes, p + 8)? as usize,
            u32_at(bytes, p + 12)? as usize,
            u32_at(bytes, p + 16)? as usize,
            u32_at(bytes, p + 20)? as usize,
        ));
    }
    let offset = |rva: usize| -> Result<usize> {
        entries
            .iter()
            .find(|(vs, va, rs, _)| rva >= *va && rva < va + vs.max(rs))
            .map(|(_, va, _, raw)| raw + rva - va)
            .context("Unmapped PE import RVA")
    };
    let mut p = offset(u32_at(bytes, dir + 8)? as usize)?;
    let mut descriptors = Vec::new();
    for _ in 0..200 {
        let name = u32_at(bytes, p + 12)? as usize;
        if name == 0 {
            break;
        }
        let name = offset(name)?;
        let tail = bytes.get(name..).context("Import name outside file")?;
        let end = tail
            .iter()
            .position(|b| *b == 0)
            .context("Unterminated import name")?;
        if &tail[..end] == b"BstNativeWindows.dll" {
            return Ok(bytes.into());
        }
        descriptors.extend_from_slice(bytes.get(p..p + 20).context("Truncated import table")?);
        p += 20;
        if descriptors.len() >= 4000 {
            bail!("Import table exceeds expected bounds")
        }
    }
    let first = entries
        .iter()
        .filter(|e| e.3 > 0)
        .map(|e| e.3)
        .min()
        .context("Missing PE sections")?;
    if sections + (count + 1) * 40 > first {
        bail!("No spare PE section header")
    }
    let virtual_end = entries
        .iter()
        .map(|(vs, va, rs, _)| va + vs.max(rs))
        .max()
        .unwrap();
    let sa = u32_at(bytes, opt + 32)? as usize;
    let fa = u32_at(bytes, opt + 36)? as usize;
    let va = align(virtual_end, sa)?;
    let mut payload = descriptors.clone();
    let descriptor = payload.len();
    payload.resize(payload.len() + 40, 0);
    while payload.len() % 8 != 0 {
        payload.push(0)
    }
    let ilt = payload.len();
    payload.resize(ilt + 16, 0);
    let iat = payload.len();
    payload.resize(iat + 16, 0);
    let name = payload.len();
    payload.extend(b"BstNativeWindows.dll\0");
    if payload.len() % 2 != 0 {
        payload.push(0)
    }
    let symbol = payload.len();
    payload.extend([0, 0]);
    payload.extend(b"NativeBridgeInitialize\0");
    payload[ilt..ilt + 8].copy_from_slice(&((va + symbol) as u64).to_le_bytes());
    payload[iat..iat + 8].copy_from_slice(&((va + symbol) as u64).to_le_bytes());
    for (i, n) in [va + ilt, 0, 0, va + name, va + iat]
        .into_iter()
        .enumerate()
    {
        put32(&mut payload, descriptor + i * 4, n as u32);
    }
    let raw = align(bytes.len(), fa)?;
    let raw_size = align(payload.len(), fa)?;
    let mut out = bytes.to_vec();
    out.resize(raw + raw_size, 0);
    out[raw..raw + payload.len()].copy_from_slice(&payload);
    let p = sections + count * 40;
    out[p..p + 8].copy_from_slice(b".bstwin\0");
    for (i, n) in [payload.len(), va, raw_size, raw, 0, 0]
        .into_iter()
        .enumerate()
    {
        put32(&mut out, p + 8 + i * 4, n as u32);
    }
    put16(&mut out, p + 32, 0);
    put16(&mut out, p + 34, 0);
    put32(&mut out, p + 36, 0xc0000040);
    put16(&mut out, coff + 2, (count + 1) as u16);
    put32(&mut out, opt + 56, align(va + payload.len(), sa)? as u32);
    put32(&mut out, opt + 64, 0);
    put32(&mut out, dir + 8, va as u32);
    put32(&mut out, dir + 12, (descriptors.len() + 40) as u32);
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pe_patch_preserves_imports_and_is_idempotent() {
        let mut bytes = vec![0; 1536];
        put32(&mut bytes, 0x3c, 0x80);
        bytes[0x80..0x84].copy_from_slice(b"PE\0\0");
        put16(&mut bytes, 0x86, 1);
        put16(&mut bytes, 0x94, 240);
        put16(&mut bytes, 0x98, 0x20b);
        put32(&mut bytes, 0xb8, 4096);
        put32(&mut bytes, 0xbc, 512);
        put32(&mut bytes, 0x110, 4096);
        for (p, n) in [
            (0x190, 1024),
            (0x194, 4096),
            (0x198, 1024),
            (0x19c, 512),
            (524, 4160),
        ] {
            put32(&mut bytes, p, n);
        }
        bytes[576..588].copy_from_slice(b"example.dll\0");
        let patched = patch_import(&bytes).unwrap();
        assert_eq!(u16_at(&patched, 0x86).unwrap(), 2);
        assert_eq!(patch_import(&patched).unwrap(), patched);
        assert!(patched.windows(12).any(|s| s == b"example.dll\0"));
        assert!(patch_import(&[0; 64]).is_err());
    }
}
