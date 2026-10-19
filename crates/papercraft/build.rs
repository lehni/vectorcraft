//! Picks the Paper.js build to embed: the vendored release (`vendor/paper-full.js`), or, with
//! `PAPERCRAFT_PAPER_JS=<file>` set, that file, e.g. the latest prebuilt `dist/paper-full.js` of
//! Paper.js's `prebuilt/module` branch while Paper.js changes are being developed. A relative path
//! is relative to the workspace. Nothing is fetched: the build reads only local files.

use std::path::PathBuf;

fn main() {
    println!("cargo::rerun-if-env-changed=PAPERCRAFT_PAPER_JS");
    let crate_dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default());
    let path = match std::env::var_os("PAPERCRAFT_PAPER_JS").filter(|path| !path.is_empty()).map(PathBuf::from) {
        Some(path) if path.is_absolute() => path,
        Some(path) => crate_dir.join("../..").join(path),
        None => crate_dir.join("vendor/paper-full.js"),
    };
    if !path.is_file() {
        println!("cargo::error=PAPERCRAFT_PAPER_JS={}: no such file", path.display());
    }
    println!("cargo::rerun-if-changed={}", path.display());
    println!("cargo::rustc-env=PAPERCRAFT_PAPER_JS_PATH={}", path.display());
}
