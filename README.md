<img src="assets/minimal-bluestack.png" width="72" alt="Minimal BlueStack icon">

# Minimal BlueStack

[![Rust](https://img.shields.io/badge/Rust-stable-DEA584?style=flat-square&logo=rust)](https://www.rust-lang.org/)
[![GPUI](https://img.shields.io/badge/GPUI-0.2.2-7C3AED?style=flat-square)](https://github.com/zed-industries/zed/tree/main/crates/gpui)
![Windows x64](https://img.shields.io/badge/Windows-x64-0078D4?style=flat-square)
[![MIT license](https://img.shields.io/github/license/rosydaqbar/minimal-bluestack?style=flat-square)](LICENSE)

[![Stars](https://img.shields.io/github/stars/rosydaqbar/minimal-bluestack?style=flat-square&logo=github)](https://github.com/rosydaqbar/minimal-bluestack/stargazers)
[![Forks](https://img.shields.io/github/forks/rosydaqbar/minimal-bluestack?style=flat-square&logo=github)](https://github.com/rosydaqbar/minimal-bluestack/forks)
[![Last commit](https://img.shields.io/github/last-commit/rosydaqbar/minimal-bluestack?style=flat-square)](https://github.com/rosydaqbar/minimal-bluestack/commits/main/)

Pop out Android games into their own desktop windows. Built with **Rust + GPUI** for your existing BlueStacks installation.

## ✨ Features

- Pop out games into separate, movable and resizable desktop windows.
- Launch each game from its own desktop shortcut, with its original icon.
- Share **one Android instance** across game windows, with the emulator interface in the background.
- Open the standard BlueStacks window when needed, then hide it again.
- List optional bloatware, clean selected items, and undo changes.
- Stop Android after the last game closes while the launcher is hidden.

## 🧩 Compatibility

| Requirement | Version |
| --- | --- |
| Windows | x64 |
| BlueStacks | **5.22.290.1031 x64** |
| Android instance | **Android 13 · Tiramisu64** |

The native bridge requires this exact BlueStacks build. Install BlueStacks and create the Android instance separately.

## 🛠️ Build

Requires Rust stable/MSVC, Visual Studio Build Tools, Windows SDK, JDK 17+, and Android SDK/NDK with platform 33+ and D8. Set `ANDROID_HOME` or `ANDROID_SDK_ROOT` for a custom SDK path.

```sh
rustup target add x86_64-linux-android
cargo xtask build
```

Open **`dist/MinimalBlueStack.exe`**. Keep the entire `dist` folder together in a writable location.

## 🚀 Quick start

1. Check the detected installation and data folders; use **Browse…** for custom paths.
2. Close BlueStacks, then choose **Install core · UAC** to back up and install the bridge.
3. Choose **Connect Android** to scan installed apps.
4. Select a game → **Open window** or **Create shortcut**.
5. Choose **Load bloatware** → review the list → **Clean**. Use **Undo** to restore recorded changes.

Use **Open BlueStacks** to access the normal emulator window, or **Hide BlueStacks** to return it to the background. Android stays running while that window is open.

Cleanup statuses: **✓ Cleared** · **– Pending** · **× Protected / Failed**, with a reason for each item. Some ad components require working Android root access.

## 📌 Limits

- Up to two pop-out windows at once, each rendered at **540 × 960**. Mouse taps/drags and **Esc** for Back; keyboard mapping and multi-touch are not implemented.
- Frames use GPU readback and Windows GDI; FPS depends on the game and hardware.
- Cleanup preserves Google services and sign-in. It does not remove ads inside games or every BlueStacks screen.

[Technical reference](docs/REFERENCE.md) · [Third-party notices](THIRD_PARTY.md) · [MIT license](LICENSE)
