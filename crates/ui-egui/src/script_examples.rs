//! The Scriptographer panel's Examples: the original Scriptographer's examples, ported to Paper.js
//! (`assets/scriptographer/examples`, MIT), built into the app. The app writes them to a folder of
//! its own, next to the preferences, again when they changed, and lists it first and sealed
//! ([`crate::scripts::set_up_script_folders`]): they run, import and open in an editor as any
//! script.

use std::path::Path;

macro_rules! examples {
    ($($path:literal,)*) => {
        &[$(($path, include_str!(concat!("../../../assets/scriptographer/examples/", $path))),)*]
    };
}

/// The examples: their paths in the folder, and their sources.
const EXAMPLES: &[(&str, &str)] = examples![
    "Scripts/Growth.js",
    "Scripts/Spiro.js",
    "Scripts/Stitch.js",
    "Scripts/Text Ranges.js",
    "Tools/Area Text.js",
    "Tools/Clone Brush.js",
    "Tools/Clouds.js",
    "Tools/Dripping Brush.js",
    "Tools/Fancy Brush.js",
    "Tools/Friction.js",
    "Tools/Grid.js",
    "Tools/Growth.js",
    "Tools/Multi Lines.js",
    "Tools/Scribbler.js",
    "Tools/Square Rounded.js",
    "Tools/Square.js",
    "Tools/Stitch.js",
    "Tools/Text on a Path.js",
    "Tools/Throw.js",
    "Tools/Tree.js",
    "Tools/Vektor.js",
    "Tools/Wave.js",
    "Tools/Weed Rounded.js",
    "Tools/Weed.js",
    "Tools/Worm Farm.js",
];

/// The file in the folder that says which examples it has.
const STAMP: &str = ".examples";

/// Writes the examples to `folder`, unless it has these already: an example changed there is
/// written over once the app's examples change. Other files in it stay.
pub(crate) fn install(folder: &Path) -> std::io::Result<()> {
    let stamp = stamp();
    if std::fs::read_to_string(folder.join(STAMP)).is_ok_and(|s| s == stamp) {
        return Ok(());
    }
    for (path, source) in EXAMPLES {
        let file = folder.join(path);
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(file, source)?;
    }
    std::fs::write(folder.join(STAMP), stamp)
}

/// What tells these examples from others: a hash of their paths and sources (FNV-1a).
fn stamp() -> String {
    let hash = EXAMPLES
        .iter()
        .flat_map(|(path, source)| path.bytes().chain(source.bytes()))
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3));
    format!("{hash:016x}")
}
