mod bootstrap;
use anyhow::{Context, Result, bail};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
fn run(command: &mut Command) -> Result<()> {
    let status = command.status()?;
    if !status.success() {
        bail!("Command failed: {command:?}")
    }
    Ok(())
}
fn newest(path: &Path) -> Result<PathBuf> {
    let mut paths = fs::read_dir(path)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.path())
        .collect::<Vec<_>>();
    paths.sort();
    paths
        .pop()
        .with_context(|| format!("No SDK component found in {}", path.display()))
}
fn cargo(args: &[&str], root: &Path) -> Result<()> {
    run(Command::new("cargo").args(args).current_dir(root))
}
fn main() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let action = std::env::args().nth(1).unwrap_or("build".into());
    if action == "icon" {
        return icon(&root);
    }
    if action == "bootstrap" {
        let output = std::env::args()
            .nth(2)
            .map(PathBuf::from)
            .unwrap_or(root.join("build-temp/MinimalBootstrap.class"));
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(output, bootstrap::class_file())?;
        return Ok(());
    }
    if action != "build" && action != "test" {
        bail!("Usage: cargo xtask [build|test|bootstrap]")
    }
    if action == "test" {
        cargo(&["test", "--locked"], &root)?;
        cargo(&["test", "-p", "minimal-core", "--locked"], &root)?;
        cargo(&["test", "-p", "minimal-native", "--locked"], &root)?;
        cargo(&["test", "-p", "xtask", "--locked"], &root)?;
        return Ok(());
    }
    let temp = root.join("build-temp");
    fs::create_dir_all(&temp)?;
    let runtime = root.join("runtime");
    fs::create_dir_all(runtime.join("core"))?;
    let sdk = std::env::var_os("ANDROID_HOME")
        .or_else(|| std::env::var_os("ANDROID_SDK_ROOT"))
        .map(PathBuf::from)
        .unwrap_or(
            PathBuf::from(std::env::var_os("LOCALAPPDATA").context("LOCALAPPDATA missing")?)
                .join("Android/Sdk"),
        );
    let ndk = newest(&sdk.join("ndk"))?;
    let tools = newest(&sdk.join("build-tools"))?;
    let platform = newest(&sdk.join("platforms"))?;
    let clang = ndk.join("toolchains/llvm/prebuilt/windows-x86_64/bin/clang.exe");
    if !clang.is_file() {
        bail!("The Windows x64 Android NDK toolchain is missing.")
    }
    cargo(&["build", "--release", "--locked"], &root)?;
    cargo(
        &["build", "--release", "--locked", "-p", "minimal-guard"],
        &root,
    )?;
    cargo(
        &["build", "--release", "--locked", "-p", "minimal-native"],
        &root,
    )?;
    run(Command::new("cargo")
        .args([
            "build",
            "--release",
            "--locked",
            "-p",
            "minimal-guest",
            "--target",
            "x86_64-linux-android",
        ])
        .current_dir(&root)
        .env("CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER", &clang)
        .env(
            "CARGO_TARGET_X86_64_LINUX_ANDROID_RUSTFLAGS",
            "-C link-arg=--target=x86_64-linux-android33",
        ))?;
    let bootstrap = temp.join("MinimalBootstrap.class");
    fs::write(&bootstrap, bootstrap::class_file())?;
    run(Command::new("java")
        .args(["-cp"])
        .arg(tools.join("lib/d8.jar"))
        .args(["com.android.tools.r8.D8", "--min-api", "33", "--lib"])
        .arg(platform.join("android.jar"))
        .arg("--output")
        .arg(runtime.join("minimal-guest.jar"))
        .arg(bootstrap))?;
    fs::copy(
        root.join("target/release/minimal_native.dll"),
        runtime.join("core/BstNativeWindows.dll"),
    )?;
    fs::copy(
        root.join("target/release/minimal-guard.exe"),
        runtime.join("MinimalGameGuard.exe"),
    )?;
    fs::copy(
        root.join("target/x86_64-linux-android/release/libminimal_guest.so"),
        runtime.join("libminimal_guest.so"),
    )?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis();
    let dist = temp.join(format!("rust-package-{stamp}"));
    fs::create_dir_all(dist.join("runtime/core"))?;
    fs::create_dir_all(dist.join("assets"))?;
    fs::copy(
        root.join("target/release/bluestacks-toolbox.exe"),
        dist.join("MinimalBlueStack.exe"),
    )?;
    // Explicit allowlist: no vendor files, personal catalogs, logs, account data or game artwork.
    for file in [
        "minimal-guest.jar",
        "libminimal_guest.so",
        "MinimalGameGuard.exe",
        "core/BstNativeWindows.dll",
    ] {
        fs::copy(runtime.join(file), dist.join("runtime").join(file))?;
    }
    let empty = b"{\"schema\":1,\"games\":[]}";
    for name in ["games.example.json", "games.json"] {
        let path = dist.join("runtime").join(name);
        if !path.exists() {
            fs::write(path, empty)?;
        }
    }
    for file in ["README.md", "LICENSE", "THIRD_PARTY.md"] {
        fs::copy(root.join(file), dist.join(file))?;
    }
    for file in [
        "minimal-bluestack.ico",
        "minimal-bluestack.png",
        "minimal-bluestack.svg",
    ] {
        fs::copy(
            root.join("assets").join(file),
            dist.join("assets").join(file),
        )?;
    }
    notices(&root, &dist.join("licenses"), &ndk)?;
    let destination = root.join("dist");
    if destination.exists() {
        fs::rename(&destination, temp.join(format!("previous-package-{stamp}")))?;
    }
    fs::rename(&dist, &destination)?;
    println!(
        "Built Rust application: {}",
        destination.join("MinimalBlueStack.exe").display()
    );
    Ok(())
}

fn icon(root: &Path) -> Result<()> {
    let svg = fs::read(root.join("assets/minimal-bluestack.svg"))?;
    let tree = resvg::usvg::Tree::from_data(&svg, &resvg::usvg::Options::default())?;
    let sizes = [16u32, 24, 32, 48, 64, 128, 256];
    let mut frames = Vec::new();
    for size in sizes {
        let mut pixels =
            resvg::tiny_skia::Pixmap::new(size, size).context("Icon allocation failed")?;
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::from_scale(size as f32 / 256., size as f32 / 256.),
            &mut pixels.as_mut(),
        );
        let png = pixels.encode_png()?;
        if size == 256 {
            fs::write(root.join("assets/minimal-bluestack.png"), &png)?;
        }
        frames.push(png);
    }
    let mut output = vec![0, 0, 1, 0, 7, 0];
    let mut offset = 6 + 7 * 16;
    for (size, frame) in sizes.iter().zip(&frames) {
        output.extend([*size as u8, *size as u8, 0, 0, 1, 0, 32, 0]);
        output.extend((frame.len() as u32).to_le_bytes());
        output.extend((offset as u32).to_le_bytes());
        offset += frame.len();
    }
    for frame in frames {
        output.extend(frame);
    }
    fs::write(root.join("assets/minimal-bluestack.ico"), output)?;
    Ok(())
}
fn notices(root: &Path, destination: &Path, ndk: &Path) -> Result<()> {
    let output = Command::new("cargo")
        .args(["metadata", "--locked", "--format-version", "1"])
        .current_dir(root)
        .output()?;
    if !output.status.success() {
        bail!("Cannot read Cargo metadata")
    }
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    fs::create_dir_all(destination)?;
    let mut index = Vec::new();
    for package in metadata["packages"]
        .as_array()
        .context("Invalid Cargo metadata")?
    {
        if package["source"].is_null() {
            continue;
        }
        let name = package["name"].as_str().unwrap_or("");
        let version = package["version"].as_str().unwrap_or("");
        let directory = Path::new(package["manifest_path"].as_str().unwrap())
            .parent()
            .unwrap();
        let target = destination.join(format!("{name}-{version}"));
        let mut names = Vec::new();
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let filename = entry.file_name().to_string_lossy().into_owned();
            if entry.file_type()?.is_file()
                && ["LICENSE", "LICENCE", "NOTICE", "COPYING", "AUTHORS"]
                    .iter()
                    .any(|prefix| filename.starts_with(prefix))
            {
                fs::create_dir_all(&target)?;
                fs::copy(entry.path(), target.join(&filename))?;
                names.push(filename);
            }
        }
        index.push(serde_json::json!({"name":name,"version":version,"license":package["license"],"notices":names}));
    }
    fs::write(
        destination.join("dependencies.json"),
        serde_json::to_vec_pretty(&index)?,
    )?;
    fs::copy(
        ndk.join("NOTICE"),
        destination.join("Android-NDK-NOTICE.txt"),
    )?;
    Ok(())
}
