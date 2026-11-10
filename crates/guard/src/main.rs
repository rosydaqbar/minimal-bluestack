#![windows_subsystem = "windows"]
use minimal_core::{backend::Backend, engine::Engine, storage, watcher};
fn main() {
    let backend = Backend::discover();
    let args = std::env::args().collect::<Vec<_>>();
    let pid = args
        .iter()
        .position(|s| s == "--pid")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok());
    if let Some(pid) = pid {
        if let Err(error) = watcher::watch(&Engine::new(&backend.root), pid) {
            storage::log(
                &backend.root,
                "watcher-events.log",
                &format!("Rust watcher failed: {error:#}"),
            );
        }
    }
}
