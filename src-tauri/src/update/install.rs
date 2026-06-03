//! Download → verify → extract → atomic swap of the server bundle
//! (`codeg-server` + `codeg-mcp` + `web/`).
//!
//! The running worker performs the swap, keeping a `.bak` of each artifact,
//! then exits so the supervisor (or a re-exec) brings up the new version.
//! Every step that touches live files happens *after* the signature is
//! verified.

use std::io::Cursor;
use std::path::{Component, Path, PathBuf};

use futures_util::StreamExt;
use serde::Serialize;

use crate::app_error::AppCommandError;
use crate::update::{verify, version};

/// Reject absurdly large archives outright. Server bundles are tens of MB;
/// this is a guard against a hostile/corrupt `Content-Length` driving an
/// unbounded allocation, not a real limit.
const MAX_ARCHIVE_BYTES: u64 = 600 * 1024 * 1024;

/// Cap on cumulative *decompressed* bytes during extraction. The compressed
/// download is bounded separately by [`MAX_ARCHIVE_BYTES`]; this stops a
/// signed-but-mispackaged (or, under key compromise, hostile) archive from
/// expanding without bound and filling the disk while it holds the update
/// lock. Real server bundles are well under this.
const MAX_EXTRACTED_BYTES: u64 = 1536 * 1024 * 1024;

/// Progress milestones surfaced to the frontend over the WS bridge.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdatePhase {
    Downloading,
    Verifying,
    Extracting,
    Swapping,
}

pub type ProgressFn<'a> = dyn Fn(UpdatePhase, u64, Option<u64>) + Send + Sync + 'a;

pub struct InstallOutcome {
    pub version: String,
}

/// Release asset basename for the current platform, matching the
/// `artifact` names produced by `.github/workflows/release.yml`.
pub fn asset_basename() -> Option<&'static str> {
    use std::env::consts::{ARCH, OS};
    Some(match (OS, ARCH) {
        ("linux", "x86_64") => "codeg-server-linux-x64",
        ("linux", "aarch64") => "codeg-server-linux-arm64",
        ("macos", "x86_64") => "codeg-server-darwin-x64",
        ("macos", "aarch64") => "codeg-server-darwin-arm64",
        ("windows", "x86_64") => "codeg-server-windows-x64",
        _ => return None,
    })
}

fn archive_ext() -> &'static str {
    if cfg!(windows) {
        ".zip"
    } else {
        ".tar.gz"
    }
}

fn server_bin_filename() -> &'static str {
    if cfg!(windows) {
        "codeg-server.exe"
    } else {
        "codeg-server"
    }
}

fn mcp_bin_filename() -> &'static str {
    if cfg!(windows) {
        "codeg-mcp.exe"
    } else {
        "codeg-mcp"
    }
}

struct Targets {
    server_bin: PathBuf,
    mcp_bin: PathBuf,
    web_dir: PathBuf,
}

fn resolve_targets() -> Result<Targets, AppCommandError> {
    let server_bin = crate::update::runtime::self_exe();
    let bindir = server_bin
        .parent()
        .ok_or_else(|| AppCommandError::io_error("Cannot resolve server binary directory"))?
        .to_path_buf();
    let mcp_bin = bindir.join(mcp_bin_filename());

    let web_dir = crate::web::find_static_dir_standalone(
        std::env::var("CODEG_STATIC_DIR").ok().as_deref(),
    );
    // Absolutize so the rename-based swap is filesystem-stable regardless of
    // the process CWD (which the supervisor/respawn may not preserve).
    let web_dir = std::fs::canonicalize(&web_dir).unwrap_or(web_dir);

    Ok(Targets {
        server_bin,
        mcp_bin,
        web_dir,
    })
}

// ─── staged-upgrade marker ────────────────────────────────────────────────
//
// A completed swap drops this marker next to the server binary. It does two
// jobs:
//   1. Tells the supervisor that the *next* worker launch is the trial of a
//      newly-swapped version (so it is put on probation and auto-rolled-back
//      if it cannot boot) — and, crucially, that a plain `restart_app` with
//      no pending upgrade is NOT a trial.
//   2. Makes a second `perform_update` refuse before the first has been
//      applied by a restart; re-swapping would overwrite the `.bak` with the
//      already-new files and destroy rollback to the original version.

fn upgrade_marker_path() -> Option<PathBuf> {
    crate::update::runtime::self_exe()
        .parent()
        .map(|d| d.join(".codeg-upgrade-staged"))
}

/// True if a swapped-but-not-yet-applied upgrade is staged.
pub fn upgrade_staged() -> bool {
    upgrade_marker_path().map(|p| p.exists()).unwrap_or(false)
}

/// Record that an upgrade has been staged (best-effort: a failure only means
/// the supervisor won't put the next launch on probation, not data loss).
fn mark_upgrade_staged() {
    if let Some(p) = upgrade_marker_path() {
        if let Err(e) = std::fs::write(&p, b"staged\n") {
            eprintln!("[update][WARN] failed to write upgrade marker: {e}");
        }
    }
}

/// Consume the staged-upgrade marker, returning whether it was present. The
/// supervisor calls this when (re)launching a worker: a present marker means
/// this launch is the trial of a freshly-swapped version.
pub fn take_upgrade_staged() -> bool {
    match upgrade_marker_path() {
        Some(p) if p.exists() => {
            let _ = std::fs::remove_file(&p);
            true
        }
        _ => false,
    }
}

/// Fail fast if we cannot write where the swap needs to land — much better
/// to abort before downloading 50 MB than to discover a read-only
/// `/usr/local/bin` halfway through.
fn preflight_writable(targets: &Targets) -> Result<(), AppCommandError> {
    let bindir = targets
        .server_bin
        .parent()
        .ok_or_else(|| AppCommandError::io_error("Cannot resolve server binary directory"))?;
    check_writable(bindir)?;
    if let Some(web_parent) = targets.web_dir.parent() {
        check_writable(web_parent)?;
    }
    Ok(())
}

fn check_writable(dir: &Path) -> Result<(), AppCommandError> {
    let probe = dir.join(format!(".codeg-write-probe-{}", std::process::id()));
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            Ok(())
        }
        Err(e) => Err(AppCommandError::permission_denied(format!(
            "Update target is not writable: {}",
            dir.display()
        ))
        .with_detail(e.to_string())),
    }
}

/// Full update: resolve targets, preflight, fetch manifest, download +
/// verify + extract the platform bundle, then atomically swap the three
/// artifacts (keeping `.bak`). On success the new files are in place and
/// the caller should trigger a restart.
pub async fn perform_update(
    data_dir: &Path,
    on_progress: &ProgressFn<'_>,
) -> Result<InstallOutcome, AppCommandError> {
    let asset = asset_basename().ok_or_else(|| {
        AppCommandError::new(
            crate::app_error::AppErrorCode::DependencyMissing,
            format!(
                "Self-update is not available for this platform ({}/{})",
                std::env::consts::OS,
                std::env::consts::ARCH
            ),
        )
    })?;

    // Refuse to stage a second upgrade on top of one that was swapped but not
    // yet applied by a restart: re-swapping would move the already-new files
    // into `.bak` and lose the ability to roll back to the original version.
    if upgrade_staged() {
        return Err(AppCommandError::already_exists(
            "An update is already staged; restart the server to apply it before updating again",
        ));
    }

    let targets = resolve_targets()?;
    preflight_writable(&targets)?;

    let manifest = version::fetch_latest_manifest().await?;
    let new_version = version::trim_v_prefix(&manifest.version).to_string();

    let ext = archive_ext();
    let archive_url = format!("{}/{}{}", version::RELEASE_DOWNLOAD_BASE, asset, ext);
    let sig_url = format!("{archive_url}.sig");

    // 1. Download archive (with progress) and its detached signature.
    let archive = download_to_vec(&archive_url, on_progress).await?;
    let sig_b64 = download_text(&sig_url).await?;

    // 2. Verify before touching anything executable.
    on_progress(UpdatePhase::Verifying, 0, None);
    verify::verify_release_signature(&archive, &sig_b64).map_err(|e| {
        AppCommandError::new(
            crate::app_error::AppErrorCode::TaskExecutionFailed,
            "Update signature verification failed",
        )
        .with_detail(e)
    })?;

    // 3. Extract into a scratch dir on the data volume.
    on_progress(UpdatePhase::Extracting, 0, None);
    let staging = data_dir.join(format!(".codeg-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(AppCommandError::io)?;
    let _cleanup = ScopedDir(staging.clone());

    extract_archive(&archive, &staging, ext)?;
    let bundle_root = find_bundle_root(&staging, asset)?;
    let new_server = bundle_root.join(server_bin_filename());
    let new_mcp = bundle_root.join(mcp_bin_filename());
    let new_web = bundle_root.join("web");
    // Require the full bundle before touching any live file. A signed but
    // mis-packaged release that dropped, say, `web/` must not be allowed to
    // install a half-new mixture (new server, stale frontend).
    if !new_server.exists() || !new_mcp.exists() || !new_web.is_dir() {
        return Err(AppCommandError::new(
            crate::app_error::AppErrorCode::TaskExecutionFailed,
            "Downloaded update is incomplete (expected codeg-server, codeg-mcp and a web/ directory)",
        ));
    }

    // 4. Swap, web → mcp → server (server last: it is the one the restart
    //    relaunches). Roll back already-swapped artifacts on any failure.
    on_progress(UpdatePhase::Swapping, 0, None);
    if new_web.is_dir() {
        replace_dir(&targets.web_dir, &new_web)?;
    }
    if new_mcp.exists() {
        if let Err(e) = replace_file(&targets.mcp_bin, &new_mcp) {
            let _ = restore_dir_from_bak(&targets.web_dir);
            return Err(e);
        }
    }
    if let Err(e) = replace_file(&targets.server_bin, &new_server) {
        let _ = restore_from_bak(&targets.mcp_bin);
        let _ = restore_dir_from_bak(&targets.web_dir);
        return Err(e);
    }

    // The swap is complete. Mark it staged so (a) the supervisor puts the
    // next launch on probation and (b) a second perform is refused until a
    // restart applies this one.
    mark_upgrade_staged();

    Ok(InstallOutcome {
        version: new_version,
    })
}

/// Restore the previous bundle from the `.bak` artifacts kept by
/// [`perform_update`]. Best-effort per artifact.
pub fn rollback() -> Result<(), AppCommandError> {
    let targets = resolve_targets()?;
    let mut restored = false;
    restored |= restore_from_bak(&targets.server_bin)?;
    restored |= restore_from_bak(&targets.mcp_bin)?;
    restored |= restore_dir_from_bak(&targets.web_dir)?;
    if !restored {
        return Err(AppCommandError::not_found(
            "No previous version is available to roll back to",
        ));
    }
    Ok(())
}

/// True when a `.bak` exists for at least one artifact (i.e. a rollback is
/// possible). Cheap enough to call from the status endpoint.
pub fn rollback_available() -> bool {
    let Ok(targets) = resolve_targets() else {
        return false;
    };
    bak_path(&targets.server_bin).exists()
}

// ─── download ────────────────────────────────────────────────────────────

async fn download_to_vec(
    url: &str,
    on_progress: &ProgressFn<'_>,
) -> Result<Vec<u8>, AppCommandError> {
    let client = version::download_client()?;
    let response = client.get(url).send().await.map_err(|e| {
        AppCommandError::network("Failed to download update package").with_detail(e.to_string())
    })?;
    if !response.status().is_success() {
        return Err(AppCommandError::network(format!(
            "Update package download returned status {}",
            response.status()
        )));
    }

    let total = response.content_length();
    if let Some(t) = total {
        if t > MAX_ARCHIVE_BYTES {
            return Err(AppCommandError::invalid_input(format!(
                "Update package is unexpectedly large ({t} bytes)"
            )));
        }
    }

    let mut downloaded: u64 = 0;
    let mut buf: Vec<u8> = Vec::with_capacity(total.unwrap_or(0).min(MAX_ARCHIVE_BYTES) as usize);
    let mut stream = response.bytes_stream();
    on_progress(UpdatePhase::Downloading, 0, total);
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| {
            AppCommandError::network("Update download interrupted").with_detail(e.to_string())
        })?;
        downloaded += chunk.len() as u64;
        if downloaded > MAX_ARCHIVE_BYTES {
            return Err(AppCommandError::invalid_input(
                "Update package exceeded the maximum allowed size",
            ));
        }
        buf.extend_from_slice(&chunk);
        on_progress(UpdatePhase::Downloading, downloaded, total);
    }
    Ok(buf)
}

async fn download_text(url: &str) -> Result<String, AppCommandError> {
    let client = version::download_client()?;
    let response = client.get(url).send().await.map_err(|e| {
        AppCommandError::network("Failed to download update signature").with_detail(e.to_string())
    })?;
    if !response.status().is_success() {
        return Err(AppCommandError::network(format!(
            "Update signature download returned status {}",
            response.status()
        )));
    }
    response.text().await.map_err(|e| {
        AppCommandError::network("Failed to read update signature").with_detail(e.to_string())
    })
}

// ─── extraction ──────────────────────────────────────────────────────────

fn extract_archive(bytes: &[u8], dest: &Path, ext: &str) -> Result<(), AppCommandError> {
    if ext == ".zip" {
        extract_zip(bytes, dest)
    } else {
        extract_tar_gz(bytes, dest)
    }
}

fn extract_tar_gz(bytes: &[u8], dest: &Path) -> Result<(), AppCommandError> {
    use flate2::read::GzDecoder;
    use tar::Archive;

    let mut archive = Archive::new(GzDecoder::new(Cursor::new(bytes)));
    let entries = archive
        .entries()
        .map_err(|e| extract_err("read tar entries", e))?;
    let mut extracted: u64 = 0;
    for entry in entries {
        let mut entry = entry.map_err(|e| extract_err("read tar entry", e))?;
        let rel = entry
            .path()
            .map_err(|e| extract_err("read tar entry path", e))?
            .into_owned();
        let safe = sanitize_entry_path(&rel)?;
        let out = dest.join(&safe);
        let etype = entry.header().entry_type();
        if etype.is_dir() {
            std::fs::create_dir_all(&out).map_err(AppCommandError::io)?;
        } else if etype.is_file() {
            // Bound cumulative decompressed output before writing anything.
            extracted = extracted.saturating_add(entry.header().size().unwrap_or(0));
            if extracted > MAX_EXTRACTED_BYTES {
                return Err(AppCommandError::invalid_input(
                    "Update archive decompresses to more than the allowed size",
                ));
            }
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent).map_err(AppCommandError::io)?;
            }
            // `tar::Entry::unpack` preserves unix mode bits, so the +x on
            // codeg-server / codeg-mcp survives.
            entry
                .unpack(&out)
                .map_err(|e| extract_err("unpack tar entry", e))?;
        } else {
            // Reject symlinks, hardlinks, devices, fifos. `unpack` would
            // materialize a symlink, letting a later entry write through it to
            // escape the staging dir before any `.bak` exists. We only ever
            // ship regular files and directories.
            return Err(AppCommandError::invalid_input(format!(
                "Update archive contains an unsupported entry type ({etype:?}): {}",
                safe.display()
            )));
        }
    }
    Ok(())
}

fn extract_zip(bytes: &[u8], dest: &Path) -> Result<(), AppCommandError> {
    use zip::ZipArchive;

    let mut archive =
        ZipArchive::new(Cursor::new(bytes)).map_err(|e| extract_err("open zip", e))?;
    let mut extracted: u64 = 0;
    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|e| extract_err("read zip entry", e))?;
        // `enclosed_name` rejects path-traversal entries by returning None.
        let Some(rel) = file.enclosed_name() else {
            return Err(AppCommandError::invalid_input(
                "Update archive contains an unsafe path entry",
            ));
        };
        let out = dest.join(rel);
        if file.is_dir() {
            std::fs::create_dir_all(&out).map_err(AppCommandError::io)?;
            continue;
        }
        // Bound cumulative decompressed output (zip-bomb guard).
        extracted = extracted.saturating_add(file.size());
        if extracted > MAX_EXTRACTED_BYTES {
            return Err(AppCommandError::invalid_input(
                "Update archive decompresses to more than the allowed size",
            ));
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(AppCommandError::io)?;
        }
        let mut writer = std::fs::File::create(&out).map_err(AppCommandError::io)?;
        std::io::copy(&mut file, &mut writer).map_err(AppCommandError::io)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Some(mode) = file.unix_mode() {
                let _ = std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode));
            }
        }
    }
    Ok(())
}

fn sanitize_entry_path(p: &Path) -> Result<PathBuf, AppCommandError> {
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::Normal(s) => out.push(s),
            Component::CurDir => {}
            _ => {
                return Err(AppCommandError::invalid_input(format!(
                    "Update archive contains an unsafe path entry: {}",
                    p.display()
                )))
            }
        }
    }
    Ok(out)
}

/// The tarball/zip wraps everything in a single `{asset}/` directory. Prefer
/// that; fall back to scanning so a future layout change doesn't break us.
fn find_bundle_root(extract_dir: &Path, asset: &str) -> Result<PathBuf, AppCommandError> {
    let server = server_bin_filename();
    let candidate = extract_dir.join(asset);
    if candidate.join(server).exists() {
        return Ok(candidate);
    }
    if extract_dir.join(server).exists() {
        return Ok(extract_dir.to_path_buf());
    }
    if let Ok(read) = std::fs::read_dir(extract_dir) {
        for entry in read.flatten() {
            let p = entry.path();
            if p.is_dir() && p.join(server).exists() {
                return Ok(p);
            }
        }
    }
    Err(AppCommandError::new(
        crate::app_error::AppErrorCode::TaskExecutionFailed,
        "Could not locate the server binary inside the update package",
    ))
}

// ─── atomic swap + rollback ──────────────────────────────────────────────

fn bak_path(target: &Path) -> PathBuf {
    let mut s = target.as_os_str().to_os_string();
    s.push(".bak");
    PathBuf::from(s)
}

/// Replace `target` with `new_src`, keeping the previous file at
/// `target.bak`. Staging happens in `target`'s own directory so the final
/// rename is same-filesystem (atomic). Renaming over a running executable is
/// fine on Linux (the inode stays open) and permitted on Windows (rename to
/// `.bak` first, then move the new file in).
fn replace_file(target: &Path, new_src: &Path) -> Result<(), AppCommandError> {
    let dir = target
        .parent()
        .ok_or_else(|| AppCommandError::io_error("Cannot resolve target directory"))?;
    let name = target
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| AppCommandError::io_error("Invalid target filename"))?;

    let staged = dir.join(format!(".{name}.new"));
    let _ = std::fs::remove_file(&staged);
    std::fs::copy(new_src, &staged).map_err(AppCommandError::io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755));
    }

    let bak = bak_path(target);
    let _ = std::fs::remove_file(&bak);
    if target.exists() {
        std::fs::rename(target, &bak).map_err(AppCommandError::io)?;
    }
    if let Err(e) = std::fs::rename(&staged, target) {
        // Best-effort un-rename so we don't leave the target missing.
        let _ = std::fs::rename(&bak, target);
        return Err(AppCommandError::io(e));
    }
    Ok(())
}

fn replace_dir(target: &Path, new_src: &Path) -> Result<(), AppCommandError> {
    let parent = target
        .parent()
        .ok_or_else(|| AppCommandError::io_error("Cannot resolve target directory"))?;
    let name = target
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| AppCommandError::io_error("Invalid target directory name"))?;

    let staged = parent.join(format!(".{name}.new"));
    let _ = std::fs::remove_dir_all(&staged);
    copy_dir_recursive(new_src, &staged)?;

    let bak = bak_path(target);
    let _ = std::fs::remove_dir_all(&bak);
    if target.exists() {
        std::fs::rename(target, &bak).map_err(AppCommandError::io)?;
    }
    if let Err(e) = std::fs::rename(&staged, target) {
        let _ = std::fs::rename(&bak, target);
        return Err(AppCommandError::io(e));
    }
    Ok(())
}

fn restore_from_bak(target: &Path) -> Result<bool, AppCommandError> {
    let bak = bak_path(target);
    if !bak.exists() {
        return Ok(false);
    }
    let _ = std::fs::remove_file(target);
    std::fs::rename(&bak, target).map_err(AppCommandError::io)?;
    Ok(true)
}

fn restore_dir_from_bak(target: &Path) -> Result<bool, AppCommandError> {
    let bak = bak_path(target);
    if !bak.exists() {
        return Ok(false);
    }
    let _ = std::fs::remove_dir_all(target);
    std::fs::rename(&bak, target).map_err(AppCommandError::io)?;
    Ok(true)
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), AppCommandError> {
    std::fs::create_dir_all(dst).map_err(AppCommandError::io)?;
    for entry in std::fs::read_dir(src).map_err(AppCommandError::io)? {
        let entry = entry.map_err(AppCommandError::io)?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let ty = entry.file_type().map_err(AppCommandError::io)?;
        if ty.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            std::fs::copy(&from, &to).map_err(AppCommandError::io)?;
        }
    }
    Ok(())
}

fn extract_err(what: &str, e: impl std::fmt::Display) -> AppCommandError {
    AppCommandError::new(
        crate::app_error::AppErrorCode::TaskExecutionFailed,
        format!("Failed to {what} from update package"),
    )
    .with_detail(e.to_string())
}

/// Removes a directory tree on drop — keeps the data volume clean even when
/// the swap errors out midway.
struct ScopedDir(PathBuf);
impl Drop for ScopedDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_rejects_parent_escape() {
        assert!(sanitize_entry_path(Path::new("../evil")).is_err());
        assert!(sanitize_entry_path(Path::new("a/../../b")).is_err());
    }

    #[test]
    fn sanitize_keeps_normal_paths() {
        let p = sanitize_entry_path(Path::new("codeg-server-linux-x64/web/index.html")).unwrap();
        assert_eq!(
            p,
            PathBuf::from("codeg-server-linux-x64/web/index.html")
        );
    }

    #[test]
    fn replace_file_keeps_backup_and_swaps() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("codeg-server");
        std::fs::write(&target, b"old").unwrap();
        let src = dir.path().join("new-bin");
        std::fs::write(&src, b"new").unwrap();

        replace_file(&target, &src).unwrap();

        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        assert_eq!(std::fs::read(bak_path(&target)).unwrap(), b"old");

        // Rollback restores the previous bytes.
        assert!(restore_from_bak(&target).unwrap());
        assert_eq!(std::fs::read(&target).unwrap(), b"old");
    }

    #[test]
    fn replace_dir_keeps_backup_and_swaps() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("web");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("index.html"), b"old").unwrap();

        let src = dir.path().join("new-web");
        std::fs::create_dir_all(src.join("assets")).unwrap();
        std::fs::write(src.join("index.html"), b"new").unwrap();
        std::fs::write(src.join("assets/app.js"), b"js").unwrap();

        replace_dir(&target, &src).unwrap();

        assert_eq!(std::fs::read(target.join("index.html")).unwrap(), b"new");
        assert_eq!(std::fs::read(target.join("assets/app.js")).unwrap(), b"js");
        assert_eq!(
            std::fs::read(bak_path(&target).join("index.html")).unwrap(),
            b"old"
        );

        assert!(restore_dir_from_bak(&target).unwrap());
        assert_eq!(std::fs::read(target.join("index.html")).unwrap(), b"old");
    }

    #[test]
    fn asset_basename_is_known_for_supported_targets() {
        // At least the host target the tests run on must resolve.
        assert!(asset_basename().is_some() || cfg!(not(any(
            target_os = "linux",
            target_os = "macos",
            target_os = "windows"
        ))));
    }
}
