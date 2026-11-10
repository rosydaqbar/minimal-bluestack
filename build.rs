fn main() {
    println!("cargo:rerun-if-changed=assets/minimal-bluestack.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let out =
            std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("app-icon.rc");
        let icon = std::fs::canonicalize("assets/minimal-bluestack.ico")
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        std::fs::write(&out, format!("1 ICON \"{icon}\"\n")).unwrap();
        embed_resource::compile(out, embed_resource::NONE)
            .manifest_required()
            .expect("embed application icon");
    }
}
