# Minimal BlueStack

A Windows manager for an independently installed BlueStacks emulator. The UI,
launcher, detection, cleanup, shortcuts, recovery, session watcher, Windows
renderer bridge and Android guest are implemented in Rust.

Android ART requires a DEX class as its entry point. The Rust build tool generates
a tiny class containing only a native loader and JNI callback declarations.
All guest behavior is implemented in the Rust library. There are no authored
Java, C++, C#, PowerShell or VBScript application sources or script dependencies.
Windows SDK and Android SDK/NDK tools remain external build prerequisites.

## Features

- Discover program and Android data folders through Windows registry records,
  uninstall entries and running processes, with saved manual folder choices.
- Browse for custom installations; show both paths and the discovery source.
- List cached apps without booting, or scan Android apps and their original icons.
- Create individual Desktop shortcuts that launch the Rust executable directly.
- Give each game's window and shortcut a matching taskbar identity and original
  app icon, so Windows does not group game windows under the emulator's icon.
- Open selected apps in independent native windows sharing one Android VM.
- Keep the emulator root transparent, click-through and off the taskbar.
- Stop an app when its window closes; synchronize storage and power off the VM
  after the last game closes. Minimized games continue running.
- Load bloatware with live cleared, pending, blocked and failed statuses, clean supported packages individually or together, and undo journaled changes.
- Install or restore the native bridge with checksum-verified backups and UAC.

The renderer ABI is version locked to **BlueStacks 5.22.290.1031 x64, Android 13,
instance Tiramisu64**. Other renderer builds are rejected. This tool does not
install BlueStacks or create the Android instance.

## Rust workspace

| Component | Responsibility |
| --- | --- |
| src/ | GPUI interface and command entry points |
| crates/core/ | Discovery, ADB, catalogs, shortcuts, cleanup, recovery and lifecycle |
| crates/guard/ | Persistent window/icon/session watcher |
| crates/bridge/ | Version-checked Win32 framebuffer renderer DLL |
| crates/guest/ | Android virtual displays, app/icon inspection and input through JNI |
| xtask/ | Build/package tooling, icon generation and ART bootstrap bytecode generation |
| assets/ | Original editable SVG and app icon assets |

## Build

Use Windows x64 with Rust stable/MSVC, Visual Studio Build Tools and a Windows
SDK. Android builds need the x86_64 Rust target, JDK 17+ with java on PATH, an
Android platform 33+, SDK build-tools with D8, and a recent Android NDK.
Set ANDROID_HOME or ANDROID_SDK_ROOT for a custom SDK directory.

~~~text
rustup target add x86_64-linux-android
cargo xtask build
~~~

Output: dist/MinimalBlueStack.exe, its runtime folder, assets and dependency
notices. Keep them together in a user-writable directory. Packaging uses an
explicit allowlist; vendor binaries, VM disks, APKs, accounts, game artwork,
personal catalogs, local preferences and logs are excluded.

D8 processes a class emitted by Rust; javac and Java source files are not used.
The Android shared library is cross-compiled from Rust with the NDK linker.
The native bridge uses renderer buffers directly: no scrcpy or video encoding.

~~~text
cargo run
cargo xtask test
cargo xtask icon
~~~

The icon task renders the original SVG into a PNG preview and a seven-size ICO
using Rust libraries. The Rust build script generates the Windows resource
description and embeds the icon. The interface and messages use English.

## Use

1. Install the supported BlueStacks build and create the Android 13 instance.
2. Open the manager and check **BlueStacks installation** and **Android data**.
   Registry detection supports custom drives. **Browse…** selects the folder
   containing HD-Player.exe and HD-Adb.exe; **Data folder…** selects the folder
   containing bluestacks.conf. **Auto-detect** resets manual choices.
3. Close all emulators before changing folders or installing the core.
   **Install core · UAC** verifies supported binaries, backs them up, adds the
   native import, enables local ADB and disables remote ADB.
4. **Connect Android** boots in the background and scans apps and icons.
5. Select an app, then **Create shortcut** or **Open window**. Each shortcut
   opens only its selected app. Scan again after installing a new game.

Closing the manager preserves open game windows. The separate Rust watcher
keeps their icons alive and stops the VM after the last game closes. Folder
changes are refused while an emulator is running. Invalid saved choices are
reported rather than silently switching to a different installation.

## Cleanup and recovery

The session watcher also keeps each running game's root task on its assigned
display. Games that switch from an SDK startup activity to a new game activity
can otherwise land on the hidden main Android screen. Placement is checked
without taking the launch/close lock away from an active operation, and shared
parent tasks containing other applications are never moved.

Cleanup uses `pm disable-user --user 0` for identified optional vendor packages
and verifies the resulting installed/disabled state. If BlueStacks silently
ignores disabling a system app, cleanup uses `pm uninstall -k --user 0` instead:
its app data and base system APK are retained. Undo uses `install-existing`
and enables the package again. This fallback is restricted to system apps, so
cleanup never removes the only APK of an ordinary installed app.
The package catalog covers vendor stores, rewards,
cloud apps, Media Manager, the vendor settings shortcut, now.gg account/payment
integrations, and the vendor Home launcher when another Home app is selected.
Each row states the affected feature: cleaning Media Manager removes its Windows
file transfer UI; vendor account/payment cleanup disables those vendor flows;
cleaning the settings shortcut removes its keyboard setup screens. The app
library is rescanned after cleanup and Undo, so removed entries disappear there
and restored entries return.
Choose **Load bloatware** to read Android user 0's current package state. The list
shows **✓ Cleared** (disabled or not installed), **– Pending** (ready to disable),
and **× Protected / Failed**, with a reason on every row. Protected dependencies
appear separately from optional cleanup entries. The system-UID command and
accessibility bridge is kept, as is the shared BS-Services package containing
GoogleSignInPopupService and BlueStacksTokenService. Eight identified advertising,
affiliate, download-statistics and promotional-popup components inside BS-Services
can be disabled individually when working Android root access is available.
Component presence and enabled state are queried from Android's package manager;
missing components are marked cleared. Root capability is checked, rather than
assuming a configuration flag grants access. Without it, component rows show
**Needs root** and explain Android's shell permission restriction. The tool does
not enable root or restart Android to change root settings.
The active Home launcher and unrecognized vendor packages remain protected.
The per-user fallback and Undo were exercised on all 13 cataloged optional
packages on BlueStacks 5.22.290.1031 / Android 13. Individual Media Manager and
vendor store cleanups were also verified. That build's shell cannot change the
eight individual ad-component states; these require a working root helper.
Sword x Staff and Seven Knights Idle Adventure were subsequently launched
separately with all 13 optional packages cleaned: their processes stayed alive
and their visible tasks stayed on the assigned native displays after startup.
The watcher was also tested by moving each running game to the hidden main
display and verifying that it returned automatically. These are startup and
display-placement checks, not validation of vendor payment flows.
Use an entry's **Clean** / **Retry** button or **Clean pending**. Statuses reload
after cleaning or undoing; loading the list starts Android in the background if
needed. `--load-bloatware --root <runtime-folder>` outputs the same report as JSON.
`--clean-item <report-id> <package-or-component>`, `--clean-bloatware <report-id>`
and `--undo-cleanup` use the same backend as the UI and output updated reports.
Pass `--root <runtime-folder>` to each command.
It preserves app data, Play Store, Google services, Google sign-in and WebView. Intent
is journaled before each change. A failure attempts rollback and reports any
rollback failure. Undo restores only packages recorded in the backup. Old app
registrations and cleanup backups remain readable after the Rust migration.

This does not remove advertisements inside third-party games or patch every
vendor UI. Game windows use this project's native window chrome.

- Cleanup backups: %LOCALAPPDATA%/MinimalBlueStack/backups.
- Per-installation core backups: %LOCALAPPDATA%/MinimalBlueStack/core-backup.
- Native logs: %LOCALAPPDATA%/MinimalBlueStack/logs.
- Catalogs, preferences, cached scans and session logs: the runtime directory.

**Restore core · UAC** restores the player and bridge backed up before setup.
It does not roll back game disks. Changed vendor binaries or damaged backups
are not overwritten automatically. Keep backups when updating or moving an
installation; do not remove them to bypass version checks.

## Limits

Two windows can be active in one instance. Arbitrarily many apps can be
registered and used in turn. Output is 540×960 per app, with mouse touch/drag and
Escape as Android Back. Frame delivery is driven by fresh Android images with
no fixed 30-fps pacing delay. An image is leased until host readback completes;
the capture mutex is not held during network I/O or input injection.
The producer's Android GPU completion fence is awaited before readback, with a
bounded wait that skips incomplete frames. Windows GDI painting is buffered and
committed after the complete client image has been drawn.
Color bitmaps explicitly use `COLORONCOLOR` scaling. The default monochrome
AND/OR scaling modes can invent dark colored grid pixels when shrinking an
image. A GDI regression test reproduces the old corruption and verifies that
resizing preserves the original color palette.
Frame-ready notifications target a message-only window and are handled by its
window procedure, so the Windows move/resize modal loop can dispatch them.
Interactive resizing has a window timer for repainting and invalidates the full
client area. Painting holds an immutable frame snapshot, releases the mailbox
lock before drawing, and flushes GDI before releasing that snapshot. A slow
paint therefore cannot hold the capture mailbox or reuse its source buffer.

Input uses a separate acknowledged channel and does not wait for frame readback.
Adjacent drag moves are coalesced while press/release boundaries are preserved;
capture loss cancels the gesture. The bridge logs delivered FPS, GPU readback
time, input-channel round-trip time, touch-dispatch time and queue delay under
Native logs. The Android broker also reports GPU-fence wait and captured-frame
age. Ping time is reported separately from actual touch dispatch.
Presentation logs separately count unique painted frames and report paint time
and frame age; a minimized window is not counted as presenting frames.

The window still uses GPU-to-CPU frame readback and GDI presentation. It is not
a direct GPU swapchain, so throughput depends on readback/presentation cost,
the Android display and the game's own frame rate. Removing the software delay
does not guarantee 60 FPS for every game. Full keyboard text, BlueStacks key
mapping, multi-touch, arbitrary sizes and more than two concurrent windows
are not implemented.

Use one runtime directory per instance. Local ADB uses a dedicated server on
port 5038 and reads the device port from bluestacks.conf. Frame data uses the
57015→9166 forward and independent input uses 57016→9167. Guest broker/control
endpoints bind only to loopback. Content-addressed guest uploads prevent a
rescan or second launch from overwriting code mapped by an active guest.

The Qt root remains mapped and transparent because hiding it can stop the
vendor renderer. A BlueStacks Windows service may remain after the VM/player
stop. Games can impose their own secondary-display/background policies.
The tool has no account system, analytics or advertising service.

## Verification and publishing

Rust tests cover custom paths, package validation, icon conversion, legacy
backups, cleanup rollback, task parsing, lifecycle/reopen races, PE import
preservation and generated bootstrap structure. Live tests additionally need
the exact supported emulator build.

Project code and original icon artwork are MIT licensed. See THIRD_PARTY.md.
BlueStacks and game assets belong to their respective owners and are not
redistributed. The gitignore excludes private state and build output. Inspect
staged files before publishing; never upload emulator installations, VM disks,
accounts or personal runtime directories.
