fn main() {
    // tauri-build tracks configuration and capabilities, but not icon inputs.
    // Regenerate the native Windows resource and embedded images on icon edits.
    println!("cargo:rerun-if-changed=icons");
    tauri_build::build()
}
