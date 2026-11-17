use super::model::{self, Pending, Release, REPO};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub fn root(home: &Path) -> PathBuf {
    home.join("updates")
}

pub fn load(home: &Path) -> Result<Option<Pending>, String> {
    let file = root(home).join("pending.json");
    if !file.exists() {
        return Ok(None);
    }
    let p: Pending = serde_json::from_slice(&std::fs::read(&file).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let package = p.path(&root(home), std::env::consts::OS, std::env::consts::ARCH)?;
    if !model::newer(&p.version, model::VERSION)? {
        let _ = std::fs::remove_file(file);
        return Ok(None);
    }
    model::verify(&package, p.size, &p.sha256)?;
    Ok(Some(p))
}

fn gh(args: &[&str], timeout: Duration) -> Result<String, String> {
    let mut cmd = crate::sys::hidden("gh");
    cmd.args(args)
        .env("GH_HOST", "github.com")
        .env("GH_PROMPT_DISABLED", "1");
    // Release JSON often exceeds pipe capacity: drain both streams while gh is alive.
    capture(cmd, timeout)
}

fn capture(cmd: std::process::Command, timeout: Duration) -> Result<String, String> {
    use std::{io::Read, process::Stdio, sync::mpsc, time::Instant};
    fn reader(mut stream: impl Read + Send + 'static) -> mpsc::Receiver<Result<Vec<u8>, String>> {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = (|| {
                let mut bytes = Vec::new();
                let mut buf = [0; 8192];
                let mut overflow = false;
                loop {
                    let n = stream.read(&mut buf).map_err(|e| e.to_string())?;
                    if n == 0 {
                        break;
                    }
                    if bytes.len() + n <= 8 * 1024 * 1024 {
                        bytes.extend_from_slice(&buf[..n]);
                    } else {
                        overflow = true;
                    }
                }
                if overflow {
                    Err("GitHub-Antwort ist zu groß".into())
                } else {
                    Ok(bytes)
                }
            })();
            let _ = tx.send(result);
        });
        rx
    }
    // A metadata/download command is small and should also work while heavy agents
    // are active. Bound its lifetime and output without applying the build memory guard.
    let mut cmd = cmd;
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let stdout = reader(child.stdout.take().ok_or("stdout fehlt")?);
    let stderr = reader(child.stderr.take().ok_or("stderr fehlt")?);
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < timeout => {
                std::thread::sleep(Duration::from_millis(50))
            }
            result => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(result
                    .err()
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| "Update-Anfrage hat zu lange gedauert".into()));
            }
        }
    };
    let out = stdout
        .recv_timeout(Duration::from_secs(2))
        .map_err(|_| "Update-Ausgabe wurde nicht geschlossen")??;
    let err = stderr
        .recv_timeout(Duration::from_secs(2))
        .map_err(|_| "Update-Fehlerausgabe wurde nicht geschlossen")??;
    if status.success() {
        Ok(String::from_utf8_lossy(&out).into_owned())
    } else {
        Err(String::from_utf8_lossy(&err).trim().to_string())
    }
}

pub fn check() -> Result<Option<Pending>, String> {
    check_for_version(model::VERSION)
}

fn check_for_version(installed: &str) -> Result<Option<Pending>, String> {
    let text = gh(
        &[
            "api",
            "--hostname",
            "github.com",
            &format!("repos/{REPO}/releases/latest"),
        ],
        Duration::from_secs(30),
    )?;
    let release: Release = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    model::select(
        &release,
        installed,
        std::env::consts::OS,
        std::env::consts::ARCH,
    )
}

pub fn download(home: &Path, p: &Pending) -> Result<(), String> {
    let path = p.path(&root(home), std::env::consts::OS, std::env::consts::ARCH)?;
    let dir = path.parent().ok_or("Download-Verzeichnis fehlt")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    // Incomplete transfers stay isolated and are never referenced by pending.json.
    let staging = tempfile::Builder::new()
        .prefix("download-")
        .tempdir_in(dir)
        .map_err(|e| e.to_string())?;
    gh(
        &[
            "release",
            "download",
            &format!("v{}", p.version),
            "--repo",
            &format!("github.com/{REPO}"),
            "--pattern",
            &p.name,
            "--dir",
            staging
                .path()
                .to_str()
                .ok_or("Download-Pfad ist kein UTF-8")?,
        ],
        Duration::from_secs(600),
    )?;
    let downloaded = staging.path().join(&p.name);
    model::verify(&downloaded, p.size, &p.sha256)?;
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| e.to_string())?;
    }
    std::fs::rename(downloaded, path).map_err(|e| e.to_string())?;
    team_core::state::write_atomic(
        &root(home).join("pending.json"),
        &serde_json::to_vec(p).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_pending_is_normal_but_malformed_pending_is_not_installable() {
        let t = tempfile::tempdir().unwrap();
        assert!(load(t.path()).unwrap().is_none());
        std::fs::create_dir(root(t.path())).unwrap();
        std::fs::write(root(t.path()).join("pending.json"), "{incomplete").unwrap();
        assert!(load(t.path()).is_err());
    }
    #[test]
    fn large_release_output_does_not_fill_a_pipe_and_deadlock() {
        let mut cmd = if cfg!(windows) {
            let mut c = crate::sys::hidden("powershell.exe");
            c.args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "$x='x'*131072; [Console]::Out.Write($x); [Console]::Error.Write($x)",
            ]);
            c
        } else {
            let mut c = crate::sys::hidden("sh");
            c.args([
                "-c",
                "head -c 131072 /dev/zero; head -c 131072 /dev/zero >&2",
            ]);
            c
        };
        cmd.stdin(std::process::Stdio::null());
        assert_eq!(capture(cmd, Duration::from_secs(10)).unwrap().len(), 131072);
    }
    #[test]
    #[ignore = "Downloads the current public package into a temporary directory; never installs"]
    fn published_package_download_matches_github_digest() {
        let p = check_for_version("0.0.0").unwrap().expect("public release");
        let dir = tempfile::tempdir().unwrap();
        download(dir.path(), &p).unwrap();
        let file = p
            .path(
                &root(dir.path()),
                std::env::consts::OS,
                std::env::consts::ARCH,
            )
            .unwrap();
        model::verify(&file, p.size, &p.sha256).unwrap();
        let stored: Pending =
            serde_json::from_slice(&std::fs::read(root(dir.path()).join("pending.json")).unwrap())
                .unwrap();
        assert_eq!(stored.sha256, p.sha256);
    }
}
