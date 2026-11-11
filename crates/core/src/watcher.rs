use crate::{
    engine::{Engine, LOCK},
    platform, storage,
};
use anyhow::Result;
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;
#[derive(Default)]
struct CloseState {
    open: [bool; 2],
    pending: [Option<u64>; 2],
    sent: [bool; 2],
    last_sent: bool,
}
impl CloseState {
    fn observe(&mut self, slot: usize, visible: bool, now: u64) {
        if visible {
            self.pending[slot] = None;
            self.sent[slot] = false;
            self.last_sent = false;
        } else if self.open[slot] {
            self.pending[slot] = Some(now);
            self.sent[slot] = false;
        }
        self.open[slot] = visible;
    }
    fn next(&self, now: u64, busy: bool, launcher_open: bool) -> Option<(usize, bool)> {
        if busy {
            return None;
        }
        let count = self.open.iter().filter(|v| **v).count();
        for slot in 0..2 {
            if self.pending[slot].is_some_and(|t| now.saturating_sub(t) >= 1000) && !self.sent[slot]
            {
                if count == 0 && !launcher_open && !self.last_sent {
                    return Some((slot, true));
                }
                if count > 0 || launcher_open {
                    return Some((slot, false));
                }
            }
        }
        None
    }
    fn commit(&mut self, slot: usize, last: bool) {
        self.sent[slot] = true;
        if last {
            self.last_sent = true;
            for i in 0..2 {
                if self.pending[i].is_some() {
                    self.sent[i] = true
                }
            }
        }
    }
}
pub fn watch(engine: &Engine, pid: u32) -> Result<()> {
    let _unique = platform::MutexGuard::lock(&format!("Local\\MinimalBlueStackRustWatch-{pid}"))?;
    let start = Instant::now();
    let mut state = CloseState::default();
    let mut icons = HashMap::<(usize, String), (isize, isize)>::new();
    let mut applied = HashMap::<isize, String>::new();
    let mut identity_errors = HashMap::<isize, String>::new();
    let mut generations = HashMap::<isize, usize>::new();
    let mut next_placement = 0;
    storage::save(
        &engine.root.join(format!("state/watch-{pid}.json")),
        &std::process::id(),
    )?;
    while platform::alive(pid) {
        let now = start.elapsed().as_millis() as u64;
        platform::hide_main(pid, &engine.title);
        let session = engine.session(pid);
        let mut open = [false; 2];
        for window in platform::windows() {
            if window.pid != pid || window.class != "BlueStacksNativeGameWindow" {
                continue;
            }
            let Some(slot) = window.slot else { continue };
            if window.generation != 0
                && generations.get(&(window.handle as isize)) != Some(&window.generation)
            {
                state.observe(slot, true, now);
                generations.insert(window.handle as isize, window.generation);
            }
            open[slot] |= window.open;
            if let Some(assignment) = session.slots.iter().find(|a| a.slot == slot) {
                if let Ok(game) = engine.game(&assignment.game) {
                    if let Ok(path) = engine.icon_path(&game) {
                        let key = path.to_string_lossy().into_owned();
                        if applied.get(&(window.handle as isize)) != Some(&key) {
                            let entry =
                                icons.entry((slot, key.clone())).or_insert_with(|| unsafe {
                                    let path = platform::wide(&path);
                                    (
                                        LoadImageW(
                                            std::ptr::null_mut(),
                                            path.as_ptr(),
                                            IMAGE_ICON,
                                            16,
                                            16,
                                            LR_LOADFROMFILE,
                                        ) as isize,
                                        LoadImageW(
                                            std::ptr::null_mut(),
                                            path.as_ptr(),
                                            IMAGE_ICON,
                                            48,
                                            48,
                                            LR_LOADFROMFILE,
                                        ) as isize,
                                    )
                                });
                            if entry.0 != 0 && entry.1 != 0 {
                                unsafe {
                                    let mut result = 0;
                                    if SendMessageTimeoutW(
                                        window.handle,
                                        WM_SETICON,
                                        ICON_SMALL as usize,
                                        entry.0,
                                        SMTO_ABORTIFHUNG,
                                        200,
                                        &mut result,
                                    ) != 0
                                        && SendMessageTimeoutW(
                                            window.handle,
                                            WM_SETICON,
                                            ICON_BIG as usize,
                                            entry.1,
                                            SMTO_ABORTIFHUNG,
                                            200,
                                            &mut result,
                                        ) != 0
                                    {
                                        match set_game_identity(engine, &game, window.handle, &path)
                                        {
                                            Ok(()) => {
                                                applied.insert(window.handle as isize, key);
                                                identity_errors.remove(&(window.handle as isize));
                                            }
                                            Err(error) => {
                                                let message = format!(
                                                    "Taskbar identity for {} failed: {error:#}",
                                                    game.name
                                                );
                                                if identity_errors.get(&(window.handle as isize))
                                                    != Some(&message)
                                                {
                                                    storage::log(
                                                        &engine.root,
                                                        "watcher-events.log",
                                                        &message,
                                                    );
                                                    identity_errors
                                                        .insert(window.handle as isize, message);
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        for (slot, value) in open.into_iter().enumerate() {
            state.observe(slot, value, now);
        }
        if now >= next_placement && state.pending.iter().all(Option::is_none) {
            next_placement = now + 1500;
            if let Err(error) = engine.reconcile_displays(pid) {
                storage::log(
                    &engine.root,
                    "watcher-events.log",
                    &format!("Display placement check failed: {error:#}"),
                );
                next_placement = now + 10000;
            }
        }
        if now < 1000 {
            let _ = storage::save(
                &engine.root.join(format!("state/watch-ready-{pid}.json")),
                &serde_json::json!({"pid":std::process::id(),"icons":applied.len()}),
            );
        }
        if let Some((slot, last)) = state.next(
            now,
            platform::MutexGuard::busy(LOCK),
            platform::launcher_visible(pid, &engine.title),
        ) {
            match engine.close(pid, if last { None } else { Some(slot) }, last) {
                Ok(()) => state.commit(slot, last),
                Err(error) => {
                    storage::log(
                        &engine.root,
                        "watcher-events.log",
                        &format!("Close failed: {error:#}"),
                    );
                    state.commit(slot, last);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    // Handles outlive every window reference. A minimized game remains open.
    for (_, (small, large)) in icons {
        unsafe {
            if small != 0 {
                DestroyIcon(small as _);
            }
            if large != 0 {
                DestroyIcon(large as _);
            }
        }
    }
    let _ = std::fs::remove_file(engine.root.join(format!("state/watch-{pid}.json")));
    Ok(())
}
pub(crate) fn set_game_identity(
    engine: &Engine,
    game: &crate::engine::Game,
    window: windows_sys::Win32::Foundation::HWND,
    icon: &std::path::Path,
) -> Result<()> {
    let launcher = [
        engine.root.join("MinimalBlueStack.exe"),
        engine
            .root
            .parent()
            .unwrap_or(&engine.root)
            .join("MinimalBlueStack.exe"),
    ]
    .into_iter()
    .find(|path| path.is_file())
    .ok_or_else(|| anyhow::anyhow!("MinimalBlueStack.exe is missing beside the runtime folder"))?;
    let command = format!(
        "{} --launch {} --root {}",
        platform::quote(&launcher.to_string_lossy()),
        game.id,
        platform::quote(&engine.root.to_string_lossy())
    );
    platform::taskbar_identity(
        window,
        &platform::game_app_id(&engine.root, &game.package),
        &game.name,
        icon,
        &command,
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn close_lifecycle_and_reopen_race() {
        let mut s = CloseState::default();
        assert_eq!(s.next(60000, false, false), None);
        s.observe(0, true, 0);
        s.observe(1, true, 0);
        s.observe(0, false, 100);
        assert_eq!(s.next(1099, false, false), None);
        assert_eq!(s.next(1100, false, false), Some((0, false)));
        s.commit(0, false);
        s.observe(1, false, 1200);
        assert_eq!(s.next(2200, true, false), None);
        s.observe(1, true, 2300);
        assert_eq!(s.next(3300, false, false), None);
        s.observe(1, false, 3400);
        assert_eq!(s.next(4400, false, false), Some((1, true)));
        s.commit(1, true);
        assert_eq!(s.next(5500, false, false), None);
    }
    #[test]
    fn closing_the_last_game_preserves_an_open_launcher() {
        let mut state = CloseState::default();
        state.observe(0, true, 0);
        state.observe(0, false, 100);
        assert_eq!(state.next(1100, false, true), Some((0, false)));
        assert_eq!(state.next(1100, false, false), Some((0, true)));
        state.commit(0, false);
        assert_eq!(state.next(2200, false, true), None);
    }
}
