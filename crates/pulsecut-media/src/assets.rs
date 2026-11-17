//! Asset packs: sound effects, meme images and fonts with explicit licensing metadata.
//!
//! A pack is a directory with `pack.json`. Every asset must carry a license (inherited from the
//! pack or set per asset); assets without one are rejected at import. The built-in pack is
//! generated locally from `sfx::BUILTIN` (original CC0 sounds) so the app works offline with
//! no third-party media.

use crate::sfx;
use pulsecut_core::paths::safe_join;
use pulsecut_core::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const BUILTIN_PACK_ID: &str = "pulsecut-core";
const MAX_ASSET_BYTES: u64 = 25 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetKind {
    Sound,
    Image,
    Font,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssetEntry {
    pub id: String,
    pub name: String,
    pub kind: AssetKind,
    /// Path relative to the pack directory.
    pub file: String,
    pub category: String,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Default gain (sound) or opacity (image), 0..2.
    #[serde(default = "one")]
    pub volume: f32,
    /// Per-asset license override (SPDX id or free text).
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub attribution: Option<String>,
}

fn one() -> f32 {
    1.0
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackManifest {
    pub id: String,
    pub name: String,
    pub author: String,
    pub version: String,
    /// SPDX id (e.g. CC0-1.0, CC-BY-4.0) or "user-owned".
    pub license: String,
    #[serde(default)]
    pub license_url: Option<String>,
    #[serde(default)]
    pub description: String,
    pub assets: Vec<AssetEntry>,
}

#[derive(Debug, Clone)]
pub struct Pack {
    pub dir: PathBuf,
    pub manifest: PackManifest,
    pub enabled: bool,
}

#[derive(Debug, Clone)]
pub struct ResolvedAsset<'a> {
    pub pack: &'a Pack,
    pub entry: &'a AssetEntry,
    pub path: PathBuf,
}

impl ResolvedAsset<'_> {
    pub fn license(&self) -> &str {
        self.entry.license.as_deref().unwrap_or(&self.pack.manifest.license)
    }
    /// Qualified id `pack/asset`.
    pub fn qid(&self) -> String {
        format!("{}/{}", self.pack.manifest.id, self.entry.id)
    }
}

#[derive(Debug, Default, Clone)]
pub struct AssetLibrary {
    pub packs: Vec<Pack>,
}

fn sniff_kind(path: &Path) -> Result<AssetKind> {
    use std::io::Read;
    let mut head = [0u8; 12];
    let mut f = std::fs::File::open(path)?;
    let n = f.read(&mut head)?;
    let h = &head[..n];
    let kind = if h.starts_with(b"RIFF") && h.len() >= 12 && &h[8..12] == b"WAVE"
        || h.starts_with(b"ID3")
        || h.len() >= 2 && h[0] == 0xFF && (h[1] & 0xE0) == 0xE0
        || h.starts_with(b"OggS")
        || h.starts_with(b"fLaC")
    {
        AssetKind::Sound
    } else if h.starts_with(&[0x89, b'P', b'N', b'G']) || h.starts_with(&[0xFF, 0xD8, 0xFF]) || h.starts_with(b"GIF8") {
        AssetKind::Image
    } else if h.starts_with(&[0, 1, 0, 0]) || h.starts_with(b"OTTO") || h.starts_with(b"true") || h.starts_with(b"ttcf")
    {
        AssetKind::Font
    } else {
        return Err(Error::other(format!(
            "{}: unsupported or unrecognized file type",
            path.display()
        )));
    };
    Ok(kind)
}

impl Pack {
    /// Load and validate a pack directory. Validation: manifest parses, license present, every
    /// file stays inside the pack, exists, is ≤ 25 MB and its magic bytes match its kind.
    pub fn load(dir: &Path) -> Result<Pack> {
        let raw = std::fs::read_to_string(dir.join("pack.json"))?;
        let manifest: PackManifest = serde_json::from_str(&raw)
            .map_err(|e| Error::other(format!("{}: invalid pack.json: {e}", dir.display())))?;
        if manifest.license.trim().is_empty() {
            return Err(Error::other(format!(
                "pack {} has no license; refusing to import",
                manifest.id
            )));
        }
        if manifest.id.is_empty()
            || !manifest
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(Error::other("pack id must be [A-Za-z0-9_-]+"));
        }
        for a in &manifest.assets {
            let p = safe_join(dir, &a.file)?;
            let meta = std::fs::metadata(&p).map_err(|e| Error::other(format!("{}: {e}", p.display())))?;
            if meta.len() > MAX_ASSET_BYTES {
                return Err(Error::other(format!("{} exceeds 25 MB", a.file)));
            }
            let k = sniff_kind(&p)?;
            if k != a.kind {
                return Err(Error::other(format!(
                    "{}: content is {:?}, manifest says {:?}",
                    a.file, k, a.kind
                )));
            }
        }
        Ok(Pack {
            dir: dir.to_path_buf(),
            manifest,
            enabled: true,
        })
    }
}

impl AssetLibrary {
    /// Load every pack under `root` (one sub-directory per pack). Invalid packs are skipped
    /// with a warning so one bad import can't break the app.
    pub fn load(root: &Path) -> AssetLibrary {
        let mut packs = vec![];
        if let Ok(rd) = std::fs::read_dir(root) {
            let mut dirs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
            dirs.sort();
            for d in dirs {
                match Pack::load(&d) {
                    Ok(p) => packs.push(p),
                    Err(e) => tracing::warn!("skipping asset pack {}: {e}", d.display()),
                }
            }
        }
        AssetLibrary { packs }
    }

    pub fn all(&self) -> impl Iterator<Item = ResolvedAsset<'_>> {
        self.packs.iter().filter(|p| p.enabled).flat_map(|p| {
            p.manifest.assets.iter().map(move |e| ResolvedAsset {
                pack: p,
                entry: e,
                path: p.dir.join(&e.file),
            })
        })
    }

    pub fn find(&self, qid: &str) -> Option<ResolvedAsset<'_>> {
        let (pack, id) = qid.split_once('/').unwrap_or((BUILTIN_PACK_ID, qid));
        self.all().find(|a| a.pack.manifest.id == pack && a.entry.id == id)
    }

    pub fn search(&self, query: &str, kind: Option<AssetKind>) -> Vec<ResolvedAsset<'_>> {
        let q = query.to_lowercase();
        self.all()
            .filter(|a| kind.is_none_or(|k| a.entry.kind == k))
            .filter(|a| {
                q.is_empty()
                    || a.entry.name.to_lowercase().contains(&q)
                    || a.entry.category.to_lowercase().contains(&q)
                    || a.entry.tags.iter().any(|t| t.to_lowercase().contains(&q))
            })
            .collect()
    }

    /// Sounds in a category, for the effect engine.
    pub fn sounds_in(&self, category: &str) -> Vec<ResolvedAsset<'_>> {
        self.all()
            .filter(|a| a.entry.kind == AssetKind::Sound && a.entry.category == category)
            .collect()
    }
}

/// Generate (or refresh) the built-in pack of original CC0 sounds under `root`.
pub fn install_builtin_pack(root: &Path) -> Result<PathBuf> {
    let dir = root.join(BUILTIN_PACK_ID);
    let sounds = dir.join("sounds");
    std::fs::create_dir_all(&sounds)?;
    let mut assets = vec![];
    for s in sfx::BUILTIN {
        let file = format!("sounds/{}.wav", s.id);
        let p = dir.join(&file);
        if !p.exists() {
            sfx::write_wav(&p, &(s.synth)())?;
        }
        assets.push(AssetEntry {
            id: s.id.into(),
            name: s.name.into(),
            kind: AssetKind::Sound,
            file,
            category: s.category.into(),
            tags: s.tags.iter().map(|t| t.to_string()).collect(),
            volume: 1.0,
            license: None,
            attribution: None,
        });
    }
    let manifest = PackManifest {
        id: BUILTIN_PACK_ID.into(),
        name: "PulseCut Essentials".into(),
        author: "PulseCut contributors".into(),
        version: "1.0.0".into(),
        license: "CC0-1.0".into(),
        license_url: Some("https://creativecommons.org/publicdomain/zero/1.0/".into()),
        description: "Original sound effects procedurally synthesized by PulseCut. No samples from third parties."
            .into(),
        assets,
    };
    std::fs::write(
        dir.join("pack.json"),
        serde_json::to_string_pretty(&manifest).map_err(|e| Error::other(e.to_string()))?,
    )?;
    Ok(dir)
}

/// Import a single user file into the `user-imports` pack with the license the user declared.
pub fn import_user_asset(
    root: &Path,
    src: &Path,
    name: &str,
    category: &str,
    license: &str,
    attribution: Option<String>,
) -> Result<String> {
    if license.trim().is_empty() {
        return Err(Error::other(
            "a license or usage-rights statement is required to import an asset",
        ));
    }
    let kind = sniff_kind(src)?;
    let meta = std::fs::metadata(src)?;
    if meta.len() > MAX_ASSET_BYTES {
        return Err(Error::other("file exceeds 25 MB"));
    }
    let dir = root.join("user-imports");
    std::fs::create_dir_all(dir.join("files"))?;
    let manifest_path = dir.join("pack.json");
    let mut manifest: PackManifest = std::fs::read_to_string(&manifest_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(PackManifest {
            id: "user-imports".into(),
            name: "My imports".into(),
            author: "me".into(),
            version: "1".into(),
            license: "user-declared".into(),
            license_url: None,
            description: "Assets imported by the user with declared usage rights".into(),
            assets: vec![],
        });
    let ext = src
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("bin")
        .to_ascii_lowercase();
    let id = pulsecut_core::paths::sanitize_file_stem(name, 40).to_ascii_lowercase();
    let id = if manifest.assets.iter().any(|a| a.id == id) {
        format!("{id}_{}", manifest.assets.len())
    } else {
        id
    };
    let file = format!("files/{id}.{ext}");
    std::fs::copy(src, dir.join(&file))?;
    manifest.assets.push(AssetEntry {
        id: id.clone(),
        name: name.into(),
        kind,
        file,
        category: category.into(),
        tags: vec![],
        volume: 1.0,
        license: Some(license.into()),
        attribution,
    });
    std::fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&manifest).map_err(|e| Error::other(e.to_string()))?,
    )?;
    Ok(format!("user-imports/{id}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_pack_loads_and_searches() {
        let d = tempfile::tempdir().unwrap();
        install_builtin_pack(d.path()).unwrap();
        let lib = AssetLibrary::load(d.path());
        assert_eq!(lib.packs.len(), 1);
        assert!(lib.find("impact_boom").is_some());
        assert!(lib.find("pulsecut-core/whoosh").is_some());
        assert!(!lib.sounds_in("comedy").is_empty());
        assert!(!lib.search("fail", Some(AssetKind::Sound)).is_empty());
        assert_eq!(lib.find("ding").unwrap().license(), "CC0-1.0");
    }

    #[test]
    fn rejects_traversal_and_mislabeled_files() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("evil");
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("x.wav"), b"not really audio").unwrap();
        let mk = |file: &str| {
            format!(
                r#"{{"id":"evil","name":"e","author":"a","version":"1","license":"CC0-1.0",
                "assets":[{{"id":"x","name":"x","kind":"sound","file":"{file}","category":"comedy"}}]}}"#
            )
        };
        std::fs::write(p.join("pack.json"), mk("../../etc/passwd")).unwrap();
        assert!(Pack::load(&p).is_err());
        std::fs::write(p.join("pack.json"), mk("x.wav")).unwrap();
        assert!(Pack::load(&p).is_err(), "fake wav must be rejected by magic bytes");
    }

    #[test]
    fn import_requires_license() {
        let d = tempfile::tempdir().unwrap();
        install_builtin_pack(d.path()).unwrap();
        let src = d.path().join("pulsecut-core/sounds/pop.wav");
        assert!(import_user_asset(d.path(), &src, "My pop", "comedy", "", None).is_err());
        let id = import_user_asset(d.path(), &src, "My pop", "comedy", "CC-BY-4.0", Some("me".into())).unwrap();
        let lib = AssetLibrary::load(d.path());
        assert_eq!(lib.find(&id).unwrap().license(), "CC-BY-4.0");
    }
}
