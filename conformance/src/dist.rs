//! Safe, nondestructive installation of the packaged public dist.
//!
//! Contract for `<OUT_DIR>`:
//! - missing or empty: created/used;
//! - an existing dist previously written by this tool (it holds
//!   [`OWNER_MARKER`] and nothing except allowlisted regular files): replaced
//!   atomically-ish (stage → rename) only after every output was produced and
//!   validated;
//! - anything else (unrelated files, subdirectories, symlinks, no marker, not a
//!   directory): refused with an error and left byte-for-byte untouched.
//!
//! Nothing is ever removed recursively and nothing outside the allowlist is
//! ever deleted. If production/validation fails, an existing dist is preserved.
use std::fs;
use std::path::{Path, PathBuf};

/// Ownership marker written into every dist this tool produces.
pub const OWNER_MARKER: &str = ".cvd-public-dist";

pub type Files = Vec<(String, Vec<u8>)>;

fn sibling(out: &Path, tag: &str) -> Result<PathBuf, String> {
    let name = out
        .file_name()
        .ok_or_else(|| format!("{}: output must name a directory", out.display()))?;
    let parent = out
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    Ok(parent.join(format!(
        ".{}.{tag}-{}",
        name.to_string_lossy(),
        std::process::id()
    )))
}

/// Refuse unsafe destinations without touching them. Returns whether `out`
/// currently holds an owned dist that will be replaced.
pub fn check_destination(out: &Path, allowed: &[&str]) -> Result<bool, String> {
    let meta = match fs::symlink_metadata(out) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(format!("{}: {e}", out.display())),
    };
    if !meta.is_dir() {
        return Err(format!(
            "{}: refusing: exists and is not a directory",
            out.display()
        ));
    }
    let mut names = Vec::new();
    for e in fs::read_dir(out).map_err(|e| format!("{}: {e}", out.display()))? {
        let e = e.map_err(|e| e.to_string())?;
        let n = e.file_name().to_string_lossy().into_owned();
        let ft = e.file_type().map_err(|e| e.to_string())?;
        if !ft.is_file() || !(n == OWNER_MARKER || allowed.contains(&n.as_str())) {
            return Err(format!(
                "{}: refusing: contains '{n}' which is not an allowlisted dist file; choose an empty or new directory",
                out.display()
            ));
        }
        names.push(n);
    }
    if names.is_empty() {
        return Ok(false);
    }
    if !names.iter().any(|n| n == OWNER_MARKER) {
        return Err(format!(
            "{}: refusing: non-empty and not a dist owned by this tool (no {OWNER_MARKER})",
            out.display()
        ));
    }
    Ok(true)
}

/// Preflight `out`, then `produce` (build + validate) all outputs, stage them
/// next to `out`, and only then replace. Errors at any point leave an existing
/// `out` unchanged.
pub fn install(
    out: &Path,
    allowed: &[&str],
    produce: impl FnOnce() -> Result<Files, String>,
) -> Result<(), String> {
    let owned = check_destination(out, allowed)?;
    let files = produce()?;
    for (n, _) in &files {
        if !allowed.contains(&n.as_str()) {
            return Err(format!("produced '{n}' is not in the allowlist"));
        }
    }
    let stage = sibling(out, "staging")?;
    fs::create_dir(&stage).map_err(|e| format!("{}: {e}", stage.display()))?;
    let staged = (|| -> Result<(), String> {
        for (n, b) in &files {
            fs::write(stage.join(n), b).map_err(|e| e.to_string())?;
        }
        fs::write(
            stage.join(OWNER_MARKER),
            b"cvd public dist (generated; safe to replace by `package`)\n",
        )
        .map_err(|e| e.to_string())
    })();
    let cleanup_stage = |dir: &Path| {
        for (n, _) in &files {
            let _ = fs::remove_file(dir.join(n));
        }
        let _ = fs::remove_file(dir.join(OWNER_MARKER));
        let _ = fs::remove_dir(dir);
    };
    if let Err(e) = staged {
        cleanup_stage(&stage);
        return Err(e);
    }
    // Re-check right before replacing (the destination may have changed).
    if let Err(e) = check_destination(out, allowed) {
        cleanup_stage(&stage);
        return Err(e);
    }
    let existed = fs::symlink_metadata(out).is_ok();
    if !existed || !owned {
        if existed {
            fs::remove_dir(out).map_err(|e| format!("{}: {e}", out.display()))?;
            // empty only
        }
        return fs::rename(&stage, out).map_err(|e| e.to_string());
    }
    let old = sibling(out, "old")?;
    fs::rename(out, &old).map_err(|e| e.to_string())?;
    if let Err(e) = fs::rename(&stage, out) {
        let _ = fs::rename(&old, out);
        cleanup_stage(&stage);
        return Err(e.to_string());
    }
    // Old dist contains only allowlisted files + marker (checked above).
    for n in allowed.iter().copied().chain([OWNER_MARKER]) {
        let _ = fs::remove_file(old.join(n));
    }
    fs::remove_dir(&old).map_err(|e| format!("{}: {e}", old.display()))
}
