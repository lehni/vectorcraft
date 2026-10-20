//! `cargo xtask corpus --wpt`: the web-platform-tests that check PaperCraft's browser APIs, from
//! the WPT repository at a pinned commit (BSD-3-Clause), checked against one sha256 over the
//! files in `xtask/wpt-corpus.sha256`. They land in `corpus/wpt/` (gitignored, never committed);
//! `vectorcraft-papercraft`'s `wpt` test runs them when present.
//!
//! A sparse, shallow checkout fetches only the folders below. Moving the pin: change the commit,
//! re-run, write the new sha256 the error prints into the manifest, and update the test's list of
//! expected failures.

use std::path::Path;
use std::process::Command;

use crate::{root, run, sha256};

const REPOSITORY: &str = "https://github.com/web-platform-tests/wpt.git";
const COMMIT: &str = "a26422aab342e2ede3dca385dbd78e16464ed58f";

/// What's fetched: the licence, the harness, the tests, and the images and font they load.
const PATHS: &[&str] = &[
    "LICENSE.md",
    "resources/testharness.js",
    "html/canvas/resources/canvas-tests.js",
    "html/canvas/offscreen",
    "css/geometry",
    "images",
    "fonts/CanvasTest.ttf",
];

const MANIFEST: &str = "xtask/wpt-corpus.sha256";

pub fn fetch() -> Result<(), String> {
    let root = root();
    let manifest = std::fs::read_to_string(root.join(MANIFEST)).map_err(|e| format!("{MANIFEST}: {e}"))?;
    let expected = manifest.split_whitespace().next().ok_or_else(|| format!("{MANIFEST} is empty"))?;
    let dest = root.join("corpus").join("wpt");
    if tree_hash(&dest).as_deref() == Ok(expected) {
        println!("WPT: verified in {}", dest.display());
        return Ok(());
    }
    if dest.exists() {
        std::fs::remove_dir_all(&dest).map_err(|e| format!("remove {}: {e}", dest.display()))?;
    }
    std::fs::create_dir_all(&dest).map_err(|e| format!("create {}: {e}", dest.display()))?;
    let git = |args: &[&str]| {
        let mut git = Command::new("git");
        git.arg("-C").arg(&dest).args(args);
        run(git, &format!("git {}", args.join(" ")))
    };
    git(&["init", "-q"])?;
    git(&["remote", "add", "origin", REPOSITORY])?;
    let patterns: Vec<String> = PATHS.iter().map(|p| format!("/{p}")).collect();
    let mut sparse = vec!["sparse-checkout", "set", "--no-cone"];
    sparse.extend(patterns.iter().map(String::as_str));
    git(&sparse)?;
    git(&["fetch", "-q", "--depth", "1", "--filter=blob:none", "origin", COMMIT])?;
    git(&["checkout", "-q", "FETCH_HEAD"])?;
    let found = tree_hash(&dest)?;
    if found != expected {
        return Err(format!("the WPT files' sha256 is {found}, {MANIFEST} has {expected}"));
    }
    println!("WPT {COMMIT}: verified in {}", dest.display());
    Ok(())
}

/// One sha256 over the fetched files: each one's path and sha256, in order.
fn tree_hash(dest: &Path) -> Result<String, String> {
    let mut files = Vec::new();
    for path in PATHS {
        collect(dest, &dest.join(path), &mut files)?;
    }
    files.sort();
    let mut listing = String::new();
    for (relative, digest) in files {
        listing.push_str(&format!("{digest}  {relative}\n"));
    }
    Ok(sha256::hex(listing.as_bytes()))
}

fn collect(dest: &Path, path: &Path, files: &mut Vec<(String, String)>) -> Result<(), String> {
    let metadata = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if metadata.is_dir() {
        let entries = std::fs::read_dir(path).map_err(|e| format!("{}: {e}", path.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("{}: {e}", path.display()))?;
            collect(dest, &entry.path(), files)?;
        }
    } else {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let relative = path.strip_prefix(dest).map_err(|e| e.to_string())?.to_string_lossy().replace('\\', "/");
        files.push((relative, sha256::hex(&bytes)));
    }
    Ok(())
}
