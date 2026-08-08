//! Locating a compiled resource across the map archive and CS2's shared content.
//!
//! `de_nuke.vpk` holds the world, its world nodes and the models baked for it,
//! but the materials and textures those models name live in the game's shared
//! archives. Mount the map first and the shared archives behind it, in the
//! order `gameinfo.gi` lists them, and answer a lookup with the first hit.
//!
//! Everything here is opened read-only. The CS2 install is never written to.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use vpk::Vpk;

/// Shared content archives, relative to the `game` directory, in search order.
const SHARED_ARCHIVES: &[&str] = &[
    "csgo/pak01_dir.vpk",
    "csgo_imported/pak01_dir.vpk",
    "csgo_core/pak01_dir.vpk",
    "core/pak01_dir.vpk",
];

/// Where CS2 installs by default. Only ever opened for reading.
pub const DEFAULT_GAME_DIR: &str =
    r"C:\Program Files (x86)\Steam\steamapps\common\Counter-Strike Global Offensive\game";

struct Mount {
    label: String,
    archive: Vpk,
}

/// Reads compiled resources from a map archive and the game's shared content.
pub struct Resolver {
    mounts: Vec<Mount>,
}

impl Resolver {
    /// Open a map archive plus every shared archive found next to it.
    ///
    /// A missing shared archive is skipped rather than fatal: a stand-alone map
    /// still loads, it just loses whatever lived in shared content.
    ///
    /// # Errors
    ///
    /// Fails only if the map archive itself cannot be opened.
    pub fn open(map_path: &Path) -> Result<Self> {
        let archive = Vpk::open(map_path)
            .with_context(|| format!("could not read VPK archive {}", map_path.display()))?;
        let mut mounts = vec![Mount {
            label: short_label(map_path),
            archive,
        }];

        if let Some(game_dir) = game_dir_for(map_path) {
            for relative in SHARED_ARCHIVES {
                let candidate = game_dir.join(relative);
                if candidate.is_file() {
                    if let Ok(shared) = Vpk::open(&candidate) {
                        mounts.push(Mount {
                            label: (*relative).to_string(),
                            archive: shared,
                        });
                    }
                }
            }
        }

        Ok(Self { mounts })
    }

    /// Labels of every mounted archive, in priority order.
    #[must_use]
    pub fn archive_labels(&self) -> Vec<&str> {
        self.mounts.iter().map(|m| m.label.as_str()).collect()
    }

    /// Whether any mounted archive holds the resource.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        let path = compiled_name(name);
        self.mounts.iter().any(|m| m.archive.find(&path).is_some())
    }

    /// Read a compiled resource by source or compiled name.
    ///
    /// # Errors
    ///
    /// Fails if no mounted archive holds the resource, or if reading it fails.
    pub fn read(&self, name: &str) -> Result<Vec<u8>> {
        let path = compiled_name(name);
        for mount in &self.mounts {
            if let Some(entry) = mount.archive.find(&path) {
                return mount
                    .archive
                    .read(entry)
                    .with_context(|| format!("reading {} from {}", entry.path, mount.label));
            }
        }
        Err(anyhow!(
            "{path} not found in any of: {}",
            self.archive_labels().join(", ")
        ))
    }
}

/// Turn a resource reference into the archive path that would hold it.
///
/// References name the *source* file (`materials/foo.vmat`); archives store the
/// *compiled* one (`materials/foo.vmat_c`). Normalise separators, lowercase,
/// and append the `_c` when it is missing — all of which Valve's loader does.
#[must_use]
pub fn compiled_name(name: &str) -> String {
    let mut path = name.replace('\\', "/").to_ascii_lowercase();
    while let Some(rest) = path.strip_prefix("./") {
        path = rest.to_string();
    }
    let path = path.trim_start_matches('/').to_string();
    if path.ends_with("_c") {
        path
    } else {
        format!("{path}_c")
    }
}

/// The `game` directory a map archive sits under, if the layout looks like an
/// install. `.../game/csgo/maps/de_nuke.vpk` yields `.../game`.
fn game_dir_for(map_path: &Path) -> Option<PathBuf> {
    let mut dir = map_path.parent()?;
    for _ in 0..3 {
        if dir.join("csgo/pak01_dir.vpk").is_file() || dir.join("core/pak01_dir.vpk").is_file() {
            return Some(dir.to_path_buf());
        }
        dir = dir.parent()?;
    }
    None
}

/// `maps/de_nuke.vpk` style label: the file plus its immediate directory.
fn short_label(path: &Path) -> String {
    let file = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    match path.parent().and_then(Path::file_name) {
        Some(parent) => format!("{}/{file}", parent.to_string_lossy()),
        None => file,
    }
}

/// The default de_nuke archive path.
#[must_use]
pub fn default_nuke_vpk() -> PathBuf {
    PathBuf::from(DEFAULT_GAME_DIR).join("csgo/maps/de_nuke.vpk")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_the_compiled_suffix_once() {
        assert_eq!(compiled_name("materials/a.vmat"), "materials/a.vmat_c");
        assert_eq!(compiled_name("materials/a.vmat_c"), "materials/a.vmat_c");
    }

    #[test]
    fn normalises_separators_case_and_prefixes() {
        assert_eq!(
            compiled_name(r"Maps\De_Nuke\Worldnodes\N0.vwnod"),
            "maps/de_nuke/worldnodes/n0.vwnod_c"
        );
        assert_eq!(compiled_name("./models/a.vmdl"), "models/a.vmdl_c");
        assert_eq!(compiled_name("/models/a.vmdl"), "models/a.vmdl_c");
    }
}
