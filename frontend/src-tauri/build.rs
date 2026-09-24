fn main() {
    // tauri-build only emits `rerun-if-changed` for config files
    // (tauri.conf.json / capabilities), NOT for the icon. Without these two
    // lines, regenerating icons/ from a changed app-icon.png would not re-run
    // this build script, and the new exe would keep the previously embedded
    // icon ("PNG 换了但 exe 图标没变").
    println!("cargo:rerun-if-changed=app-icon.png");
    println!("cargo:rerun-if-changed=icons/icon.ico");
    tauri_build::build()
}
