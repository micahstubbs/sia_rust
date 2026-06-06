//! Compiled-in package data: the bundled provider/profile JSON defaults and the
//! web visualizer's static files. In Python these ship as package-data and are
//! read via `importlib.resources`; here they are embedded at build time.

use include_dir::{include_dir, Dir};

/// Bundled `sia/defaults/` tree (providers/ and profiles/).
pub static DEFAULTS: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/sia/defaults");

/// Bundled `sia/web/static/` tree (index.html, ...).
pub static WEB_STATIC: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/sia/web/static");

/// Read a bundled default file `defaults/<subdir>/<filename>` as text, if present.
pub fn bundled_default_text(subdir: &str, filename: &str) -> Option<&'static str> {
    let path = format!("{subdir}/{filename}");
    DEFAULTS.get_file(&path).and_then(|f| f.contents_utf8())
}

/// List the `*.json` filenames bundled under `defaults/<subdir>/`.
pub fn bundled_default_filenames(subdir: &str) -> Vec<String> {
    match DEFAULTS.get_dir(subdir) {
        Some(dir) => dir
            .files()
            .filter_map(|f| {
                f.path()
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
            })
            .filter(|n| n.ends_with(".json"))
            .collect(),
        None => Vec::new(),
    }
}

/// Read a bundled web static file as bytes, if present.
pub fn web_static_bytes(name: &str) -> Option<&'static [u8]> {
    WEB_STATIC.get_file(name).map(|f| f.contents())
}
