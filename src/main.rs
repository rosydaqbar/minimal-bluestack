#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod backend;
mod ui;
use backend::Backend;
use gpui::{prelude::*, *};
use minimal_core::{engine, setup, storage, watcher};

fn main() {
    let backend = Backend::discover();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(result) = cli(&backend, &args) {
        if let Err(error) = result {
            storage::log(&backend.root, "launcher.log", &format!("FAILED: {error:#}"));
            eprintln!("{error:#}");
            if args.first().is_some_and(|a| a == "--launch")
                && !args.iter().any(|a| a == "--background")
            {
                minimal_core::platform::show_error(&format!(
                    "{error:#}\n\nLog: {}",
                    backend.root.join("launcher.log").display()
                ));
            }
            std::process::exit(1)
        }
        return;
    }
    if args.first().map(String::as_str) == Some("--diagnose") {
        match backend.scan() {
            Ok(snapshot) => println!("{}", serde_json::to_string_pretty(&snapshot).unwrap()),
            Err(error) => {
                eprintln!("{error:#}");
                std::process::exit(1)
            }
        }
        return;
    }
    let focus = !args.iter().any(|arg| arg == "--no-focus");
    Application::new().run(move |cx: &mut App| {
        let cleanup = backend.clone();
        cx.on_window_closed(move |cx| {
            if cx.windows().is_empty() {
                cleanup.stop_idle_on_exit();
                cx.quit();
            }
        })
        .detach();
        let bounds = Bounds::centered(None, size(px(1240.), px(820.)), cx);
        cx.open_window(
            WindowOptions {
                focus,
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Minimal BlueStack".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            move |_, cx| cx.new(|cx| ui::Toolbox::new(backend, cx)),
        )
        .expect("open toolbox window");
        if focus {
            cx.activate(true);
        }
    });
}

fn cli(backend: &Backend, args: &[String]) -> Option<anyhow::Result<()>> {
    let command = args.first()?.as_str();
    let engine = engine::Engine::new(&backend.root);
    Some(match command {
        "--clean-item" | "--clean-bloatware" | "--undo-cleanup" => (|| -> anyhow::Result<()> {
            let job = if command == "--undo-cleanup" {
                backend::Job::Undo
            } else {
                let id = args
                    .get(1)
                    .filter(|arg| !arg.starts_with("--"))
                    .ok_or_else(|| {
                        anyhow::anyhow!("Load bloatware first and specify its report ID.")
                    })?
                    .clone();
                if command == "--clean-item" {
                    let target = args
                        .get(2)
                        .filter(|arg| !arg.starts_with("--"))
                        .ok_or_else(|| {
                            anyhow::anyhow!("Specify the package or component from the report.")
                        })?
                        .clone();
                    backend::Job::CleanPackage(id, target)
                } else {
                    backend::Job::Apply(id)
                }
            };
            let outcome = backend.execute(job)?;
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &serde_json::json!({"message": outcome.message, "plan": outcome.plan})
                )?
            );
            if outcome.message.starts_with("Cleanup failed:")
                || outcome.message.starts_with("Undo failed:")
            {
                anyhow::bail!("{}", outcome.message)
            }
            Ok(())
        })(),
        "--load-bloatware" => backend
            .execute(backend::Job::LoadBloatware)
            .and_then(|outcome| {
                println!("{}", serde_json::to_string_pretty(&outcome.plan)?);
                Ok(())
            }),
        "--scan-live" => engine.scan_live(),
        "--boot-check" => engine.boot().map(|_| ()),
        "--open-bluestacks" => engine.open_launcher(),
        "--hide-bluestacks" => engine.hide_launcher(),
        "--refresh-shortcuts" => engine.catalog().and_then(|catalog| {
            for game in catalog.games {
                let app = backend::AndroidApp {
                    name: game.name.clone(),
                    package: game.package.clone(),
                    activity: game.activity.clone(),
                    icon: engine.icon_path(&game)?.to_string_lossy().into(),
                    enabled: true,
                    ..Default::default()
                };
                engine.register(&app, true)?;
            }
            Ok(())
        }),
        "--launch" => args
            .get(1)
            .ok_or_else(|| anyhow::anyhow!("Select an app ID"))
            .and_then(|id| engine.launch(id, args.iter().any(|a| a == "--background"))),
        "--watch" => args
            .get(1)
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| anyhow::anyhow!("Missing player PID"))
            .and_then(|pid| watcher::watch(&engine, pid)),
        "--stop-idle" => engine.stop_idle(),
        "--prepare-update" => engine.prepare_update(),
        "--setup-core" => setup::request(&backend.root, args.iter().any(|a| a == "--restore")),
        "--install-core" => setup::install(&backend.root, args.iter().any(|a| a == "--restore")),
        _ => return None,
    })
}
