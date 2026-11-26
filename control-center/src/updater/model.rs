#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

pub const VERSION: &str = env!("LUNIMA_TEAM_VERSION");
pub const REPO: &str = "aignermax/lunima-agent-loop";

#[derive(Deserialize)]
pub struct Release {
    pub tag_name: String,
    pub draft: bool,
    pub prerelease: bool,
    pub assets: Vec<Asset>,
}

#[derive(Deserialize)]
pub struct Asset {
    pub name: String,
    pub size: u64,
    pub digest: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pending {
    pub version: String,
    pub name: String,
    pub sha256: String,
    pub size: u64,
}

fn version(raw: &str) -> Result<semver::Version, String> {
    semver::Version::parse(raw.strip_prefix('v').unwrap_or(raw))
        .map_err(|e| format!("Ungültige Release-Version: {e}"))
}

pub fn asset_name(v: &str, os: &str, arch: &str) -> Result<String, String> {
    let parsed = version(v)?;
    if !parsed.pre.is_empty() || !parsed.build.is_empty() || parsed.to_string() != v {
        return Err("Nur stabile Release-Versionen werden installiert".into());
    }
    match (os, arch) {
        ("windows", "x86_64") => Ok(format!("lunima-team-{v}-win-x64.msi")),
        ("windows", "aarch64") => Ok(format!("lunima-team-{v}-win-arm64.msi")),
        ("linux", "x86_64") => Ok(format!("lunima-team_{v}_amd64.deb")),
        ("linux", "aarch64") => Ok(format!("lunima-team_{v}_arm64.deb")),
        _ => Err("Für diese Plattform ist kein Installer verfügbar".into()),
    }
}

pub fn newer(candidate: &str, installed: &str) -> Result<bool, String> {
    Ok(version(candidate)? > version(installed)?)
}

pub fn select(
    release: &Release,
    installed: &str,
    os: &str,
    arch: &str,
) -> Result<Option<Pending>, String> {
    if release.draft || release.prerelease || !newer(&release.tag_name, installed)? {
        return Ok(None);
    }
    let v = version(&release.tag_name)?.to_string();
    let name = asset_name(&v, os, arch)?;
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == name)
        .ok_or_else(|| format!("Release enthält noch kein {name}"))?;
    let hash = asset
        .digest
        .as_deref()
        .and_then(|d| d.strip_prefix("sha256:"))
        .filter(|h| valid_hash(h))
        .ok_or("Release enthält keine gültige SHA-256-Prüfsumme")?;
    if asset.size == 0 {
        return Err("Release-Paket ist leer".into());
    }
    Ok(Some(Pending {
        version: v,
        name,
        sha256: hash.to_lowercase(),
        size: asset.size,
    }))
}

fn valid_hash(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())
}

impl Pending {
    pub fn path(&self, cache: &Path, os: &str, arch: &str) -> Result<PathBuf, String> {
        if self.name != asset_name(&self.version, os, arch)?
            || !valid_hash(&self.sha256)
            || self.size == 0
        {
            return Err("Ungültiges vorbereitetes Update".into());
        }
        Ok(cache.join(&self.version).join(&self.name))
    }
}

pub fn verify(path: &Path, size: u64, expected: &str) -> Result<(), String> {
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    if file.metadata().map_err(|e| e.to_string())?.len() != size {
        return Err("Update-Download ist unvollständig".into());
    }
    let mut hash = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = file.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    if format!("{:x}", hash.finalize()) != expected.to_lowercase() {
        return Err("Update-Prüfsumme stimmt nicht überein".into());
    }
    Ok(())
}
