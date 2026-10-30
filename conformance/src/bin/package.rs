//! Reproducible public release packaging (Rust-first, no shell pipeline).
//!
//!   cargo run --locked --release -p cvd-indicators-conformance --bin package -- <OUT_DIR> [--version vX.Y.Z]
//!
//! 1. Builds ONLY the public allowlist (`PUBLIC`) for wasm32-unknown-unknown
//!    with `--profile wasm-release`, `--locked`, the 512 MiB memory maximum and
//!    path remapping (no local/CI absolute paths inside the artifacts).
//! 2. Loads every artifact through the cvd-wasm v1 harness (ABI 1, exports,
//!    declared memory max, descriptor id/version).
//! 3. Writes `<OUT_DIR>/{<file>.wasm, manifest.json, SHA256SUMS}` (+ the
//!    `.cvd-public-dist` ownership marker): exactly the files that may be
//!    published. Nothing else from target/ is ever copied.
//!
//! `<OUT_DIR>` must be new, empty, or a dist previously written by this tool;
//! anything else is refused untouched BEFORE building. Outputs are built and
//! validated first, staged beside `<OUT_DIR>`, then swapped in; a failure
//! preserves the previous dist (see `cvd_indicators_conformance::dist`).
//!
//! The SDK source is read from Cargo.lock. A local development override
//! (`CVD_SDK_PATH`, which adds a Cargo `[patch]` via `--config`) is recorded as
//! `local-override` and refused unless `--allow-local-sdk` is given; a release
//! must use the immutable public cvd-core git pin.
use cvd_indicators_conformance::{dist, sha256_hex, WasmPlugin, PUBLIC, STATUS_OK};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::Command;

const SDK_GIT: &str = "https://github.com/k0Iry/cvd-core.git";
const MAX_MEMORY_FLAG: &str = "link-arg=--max-memory=536870912";

fn sdk_source(root: &Path) -> Option<String> {
    let lock = std::fs::read_to_string(root.join("Cargo.lock")).expect("Cargo.lock");
    let mut in_sdk = false;
    for l in lock.lines() {
        if l == "[[package]]" {
            in_sdk = false;
        } else if l == "name = \"cvd-indicator-sdk\"" {
            in_sdk = true;
        } else if in_sdk {
            if let Some(s) = l.strip_prefix("source = \"") {
                return Some(s.trim_end_matches('"').to_string());
            }
        }
    }
    None
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = PathBuf::from(
        args.first()
            .filter(|a| !a.starts_with("--"))
            .expect("usage: package <OUT_DIR> [--version V] [--allow-local-sdk]"),
    );
    let flag = |n: &str| args.iter().position(|a| a == n);
    let allow_local = flag("--allow-local-sdk").is_some();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let pkg_version = env!("CARGO_PKG_VERSION");
    let version = flag("--version")
        .map(|i| args[i + 1].clone())
        .unwrap_or_else(|| format!("v{pkg_version}-dev"));

    let allowed: Vec<&str> = PUBLIC
        .iter()
        .map(|p| p.1)
        .chain(["manifest.json", "SHA256SUMS"])
        .collect();
    let result = dist::install(&out, &allowed, || {
        let sdk_override = std::env::var("CVD_SDK_PATH").ok().filter(|s| !s.is_empty());
        let mut cargo = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
        cargo.current_dir(&root);
        if let Some(p) = &sdk_override {
            cargo.arg(format!(
                "--config=patch.\"{SDK_GIT}\".cvd-indicator-sdk.path=\"{p}\""
            ));
        }
        cargo.args([
            "build",
            "--locked",
            "--profile",
            "wasm-release",
            "--target",
            "wasm32-unknown-unknown",
        ]);
        for (p, _, _) in PUBLIC {
            cargo.args(["-p", p]);
        }
        // RUSTFLAGS replaces .cargo/config.toml rustflags, so repeat the memory cap.
        let home = std::env::var("CARGO_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".cargo")
            });
        let mut flags = vec!["-C".to_string(), MAX_MEMORY_FLAG.to_string()];
        flags.push(format!("--remap-path-prefix={}=/cargo", home.display()));
        flags.push(format!("--remap-path-prefix={}=/src", root.display()));
        if let Some(p) = &sdk_override {
            flags.push(format!("--remap-path-prefix={p}=/cvd-indicator-sdk"));
        }
        cargo
            .env("CARGO_ENCODED_RUSTFLAGS", flags.join("\x1f"))
            .env_remove("RUSTFLAGS");
        let st = cargo.status().expect("spawn cargo");
        assert!(st.success(), "wasm build failed");

        let source = match (&sdk_override, sdk_source(&root)) {
            (Some(_), _) | (None, None) => {
                assert!(allow_local, "SDK is a local override; release packaging requires the public git pin (or pass --allow-local-sdk for development)");
                "local-override (development only, not publishable)".to_string()
            }
            (None, Some(s)) => {
                assert!(
                    s.starts_with(&format!("git+{SDK_GIT}?rev=")),
                    "SDK must come from the immutable public cvd-core git pin, got {s}"
                );
                s
            }
        };

        let target = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join("target"));
        let wasm_dir = target.join("wasm32-unknown-unknown/wasm-release");
        let mut files: dist::Files = Vec::new();
        let mut artifacts = Vec::new();
        let mut sums = String::new();
        for (package, file, id) in PUBLIC {
            let bytes = std::fs::read(wasm_dir.join(file)).expect("artifact");
            let mut m = WasmPlugin::load(&bytes);
            assert_eq!(m.abi, 1, "{file}: wasm abi");
            let max = m.memory_max_pages().expect("declared memory max");
            assert!(max <= 8192);
            let (s, d) = m.describe();
            assert_eq!(s, STATUS_OK);
            let d: cvd_indicator_sdk::Descriptor = serde_json::from_slice(&d).unwrap();
            assert_eq!(d.id, id);
            let text = String::from_utf8_lossy(&bytes);
            for leak in [
                root.to_string_lossy().as_ref(),
                home.to_string_lossy().as_ref(),
            ] {
                assert!(
                    leak.len() < 2 || !text.contains(leak),
                    "{file} embeds local path {leak}"
                );
            }
            let sha = sha256_hex(&bytes);
            files.push((file.to_string(), bytes.clone()));
            sums.push_str(&format!("{sha}  {file}\n"));
            artifacts.push(json!({
                "file": file, "package": package, "id": d.id, "name": d.name, "version": d.version,
                "descriptor_abi": d.abi, "wasm_abi": m.abi, "max_memory_pages": max, "bytes": bytes.len(), "sha256": sha,
            }));
        }
        let manifest = json!({
            "format": "cvd-public-indicators-manifest/1",
            "repository": "https://github.com/k0Iry/cvd-indicators",
            "release": version,
            "abi": {"wasm": 1, "descriptor": 1, "target": "wasm32-unknown-unknown", "profile": "wasm-release"},
            "sdk": {"crate": "cvd-indicator-sdk", "source": source},
            "toolchain": String::from_utf8_lossy(&Command::new("rustc").arg("-V").output().map(|o| o.stdout).unwrap_or_default()).trim(),
            "artifacts": artifacts,
        });
        let mj = serde_json::to_vec_pretty(&manifest).unwrap();
        sums.push_str(&format!("{}  manifest.json\n", sha256_hex(&mj)));
        println!("{}", String::from_utf8_lossy(&mj));
        files.push(("manifest.json".into(), mj));
        files.push(("SHA256SUMS".into(), sums.into_bytes()));
        Ok(files)
    });
    if let Err(e) = result {
        eprintln!("package: {e}");
        std::process::exit(2);
    }
}
