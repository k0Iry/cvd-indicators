//! Local acceptance of scripts/publish-dev.sh (the CI `dev` publisher) against
//! a controlled mock `gh`/`git` (tests/mock-gh). Proves the command/ref/payload
//! construction and every refusal path; it is NOT evidence of a real release.
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const SHA: &str = "1111111111111111111111111111111111111111";
const OLD: &str = "0000000000000000000000000000000000000000";
const STABLE: &str = "2222222222222222222222222222222222222222";
const WASM: [&str; 5] = [
    "cvd_indicator_cvd.wasm",
    "cvd_indicator_cvd_anchored.wasm",
    "cvd_indicator_example_range.wasm",
    "cvd_indicator_flow.wasm",
    "cvd_indicator_orderbook.wasm",
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}
fn lock_sdk() -> String {
    let lock = fs::read_to_string(root().join("Cargo.lock")).unwrap();
    let i = lock.find("name = \"cvd-indicator-sdk\"").unwrap();
    let s = &lock[i..];
    let s = &s[s.find("source = \"").unwrap() + 10..];
    s[..s.find('"').unwrap()].to_string()
}

struct Env {
    dir: PathBuf,
}
type Snap = std::collections::BTreeMap<String, Vec<u8>>;
impl Env {
    fn new(tag: &str) -> Env {
        let base = std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let dir = base.join(format!("cvd-publish-dev-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir); // our own uniquely named disposable dir
        for d in ["mock/releases", "mock/aid", "mock/tags", "dist"] {
            fs::create_dir_all(dir.join(d)).unwrap();
        }
        let e = Env { dir };
        // an existing formal (Latest) release that must never change
        e.seed(
            1,
            "v0.1.0",
            STABLE,
            false,
            &[("cvd_indicator_cvd.wasm", b"stable")],
        );
        e.w("mock/latest", b"v0.1.0");
        e.w("mock/next", b"100");
        e.w("mock/head", SHA.as_bytes());
        e.w("mock/log", b"");
        e.dist("dev", SHA, &lock_sdk(), "new");
        e
    }
    /// A published release `id` on `tag` -> `sha` with the given assets.
    fn seed(&self, id: u32, tag: &str, sha: &str, pre: bool, assets: &[(&str, &[u8])]) {
        let d = format!("mock/releases/{id}");
        fs::create_dir_all(self.p(&format!("{d}/assets"))).unwrap();
        self.w(&format!("{d}/tag"), format!("{tag}\n").as_bytes());
        self.w(&format!("{d}/target"), format!("{sha}\n").as_bytes());
        self.w(&format!("{d}/draft"), b"false\n");
        self.w(&format!("{d}/prerelease"), format!("{pre}\n").as_bytes());
        self.w(&format!("mock/tags/{tag}"), format!("{sha}\n").as_bytes());
        for (i, (n, b)) in assets.iter().enumerate() {
            self.w(&format!("{d}/assets/{n}"), b);
            self.w(
                &format!("mock/aid/{}", id * 10 + i as u32),
                format!("{id}/{n}\n").as_bytes(),
            );
        }
    }
    /// Existing public dev (release 50) for OLD with a complete old asset set.
    fn seed_old_dev(&self) {
        let names: Vec<String> = WASM
            .iter()
            .map(|s| s.to_string())
            .chain(["manifest.json".into(), "SHA256SUMS".into()])
            .collect();
        let a: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), &b"old"[..])).collect();
        self.seed(50, "dev", OLD, true, &a);
    }
    fn p(&self, rel: &str) -> PathBuf {
        self.dir.join(rel)
    }
    fn w(&self, rel: &str, b: &[u8]) {
        fs::write(self.p(rel), b).unwrap();
    }
    fn r(&self, rel: &str) -> String {
        fs::read_to_string(self.p(rel)).unwrap()
    }
    /// Complete remote state (releases, drafts, assets, tags, Latest).
    fn snap(&self) -> Snap {
        fn walk(base: &Path, d: &Path, out: &mut Snap) {
            for e in fs::read_dir(d).unwrap() {
                let p = e.unwrap().path();
                let k = p.strip_prefix(base).unwrap().to_string_lossy().into_owned();
                if p.is_dir() {
                    walk(base, &p, out);
                } else if ![
                    "log",
                    "next",
                    "head",
                    "head-after",
                    "fail",
                    "corrupt-upload",
                    "corrupt-public",
                ]
                .contains(&k.as_str())
                {
                    out.insert(k, fs::read(&p).unwrap());
                }
            }
        }
        let mut m = Snap::new();
        walk(&self.p("mock"), &self.p("mock"), &mut m);
        m
    }
    /// Write a consistent six-file dist into `dist/`.
    fn dist(&self, release: &str, commit: &str, sdk: &str, salt: &str) {
        use cvd_indicators_conformance::sha256_hex;
        let mut sums = String::new();
        for f in WASM {
            let b = format!("{f}-{salt}");
            self.w(&format!("dist/{f}"), b.as_bytes());
            sums += &format!("{}  {f}\n", sha256_hex(b.as_bytes()));
        }
        let m = serde_json::json!({"release": release, "source": {"commit": commit}, "sdk": {"crate": "cvd-indicator-sdk", "source": sdk}});
        let m = serde_json::to_vec_pretty(&m).unwrap();
        sums += &format!("{}  manifest.json\n", sha256_hex(&m));
        self.w("dist/manifest.json", &m);
        self.w("dist/SHA256SUMS", sums.as_bytes());
    }
    fn run(&self) -> (bool, String) {
        let mock = root().join("conformance/tests/mock-gh");
        let path = format!(
            "{}:{}",
            mock.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let o = Command::new("bash")
            .arg(root().join("scripts/publish-dev.sh"))
            .arg(self.p("dist"))
            .current_dir(root())
            .env("PATH", path)
            .env("MOCK_GH", self.p("mock"))
            .env("GITHUB_SHA", SHA)
            .env("GITHUB_REPOSITORY", "k0Iry/cvd-indicators")
            .env_remove("GH_TOKEN")
            .output()
            .expect("bash");
        let out = format!(
            "{}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
        (o.status.success(), out)
    }
    fn writes(&self) -> Vec<String> {
        self.r("mock/log")
            .lines()
            .filter(|l| l.contains(" -X "))
            .map(str::to_string)
            .collect()
    }
    fn releases(&self) -> Vec<(String, String, bool)> {
        let mut v: Vec<_> = fs::read_dir(self.p("mock/releases"))
            .unwrap()
            .map(|e| {
                let id = e.unwrap().file_name().to_string_lossy().into_owned();
                let g = |f: &str| {
                    self.r(&format!("mock/releases/{id}/{f}"))
                        .trim()
                        .to_string()
                };
                (id.clone(), g("tag"), g("draft") == "true")
            })
            .collect();
        v.sort();
        v
    }
    /// The single published `dev` serves exactly the dist bytes for SHA.
    fn assert_published(&self) {
        assert_eq!(
            self.r("mock/tags/dev"),
            format!("{SHA}\n"),
            "dev tag target"
        );
        let rel = self.releases();
        let devs: Vec<_> = rel.iter().filter(|r| r.1 == "dev" && !r.2).collect();
        assert_eq!(devs.len(), 1, "{rel:?}");
        assert!(rel.iter().all(|r| !r.2), "no drafts left: {rel:?}");
        let id = &devs[0].0;
        assert_eq!(
            self.r(&format!("mock/releases/{id}/prerelease")).trim(),
            "true"
        );
        let mut want: Vec<String> = WASM.iter().map(|s| s.to_string()).collect();
        want.extend(["SHA256SUMS".into(), "manifest.json".into()]);
        want.sort();
        let mut got: Vec<_> = fs::read_dir(self.p(&format!("mock/releases/{id}/assets")))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        got.sort();
        assert_eq!(got, want, "exact asset set");
        for f in &want {
            assert_eq!(
                fs::read(self.p(&format!("mock/releases/{id}/assets/{f}"))).unwrap(),
                fs::read(self.p(&format!("dist/{f}"))).unwrap(),
                "{f} bytes"
            );
        }
        self.assert_stable_untouched();
    }
    fn assert_stable_untouched(&self) {
        assert_eq!(self.r("mock/releases/1/tag").trim(), "v0.1.0");
        assert_eq!(self.r("mock/releases/1/draft").trim(), "false");
        assert_eq!(
            self.r("mock/releases/1/assets/cvd_indicator_cvd.wasm"),
            "stable"
        );
        assert_eq!(self.r("mock/tags/v0.1.0").trim(), STABLE);
        assert_eq!(self.r("mock/latest"), "v0.1.0");
        assert!(
            !self
                .writes()
                .iter()
                .any(|l| l.contains("releases/1 ") || l.contains("v0.1.0")),
            "{:?}",
            self.writes()
        );
    }
}
impl Drop for Env {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn first_publication_publishes_verified_draft_then_rerun_is_noop() {
    let e = Env::new("create");
    let (ok, out) = e.run();
    assert!(ok, "{out}");
    e.assert_published();
    let w = e.writes();
    assert!(
        w[0].contains("-X POST repos/k0Iry/cvd-indicators/releases -f tag_name=dev-staging-")
            && w[0].contains("-F draft=true -F prerelease=true -f make_latest=false"),
        "{w:?}"
    );
    assert!(
        w.iter().any(|l| l.contains(&format!(
            "-X POST repos/k0Iry/cvd-indicators/git/refs -f ref=refs/tags/dev -f sha={SHA}"
        ))),
        "{w:?}"
    );
    // idempotent: an identical rerun reads but never writes
    let before = e.snap();
    fs::write(e.p("mock/log"), "").unwrap();
    let (ok, out) = e.run();
    assert!(ok && out.contains("already publishes"), "{out}");
    assert!(e.writes().is_empty(), "{:?}", e.writes());
    assert_eq!(e.snap(), before);
}

#[test]
fn existing_dev_is_replaced_by_a_new_complete_release() {
    let e = Env::new("update");
    e.seed_old_dev();
    let (ok, out) = e.run();
    assert!(ok, "{out}");
    e.assert_published();
    assert!(
        !e.p("mock/releases/50").exists(),
        "previous dev removed after success"
    );
    let w = e.writes();
    let i = |pat: &str| {
        w.iter()
            .position(|l| l.contains(pat))
            .unwrap_or_else(|| panic!("{pat}: {w:?}"))
    };
    // all six uploaded and verified into the draft before the old dev is hidden
    assert!(i("assets?name=manifest.json") < i("releases/50 -F draft=true"));
    assert!(
        i("releases/50 -F draft=true")
            < i(&format!("git/refs/tags/dev -f sha={SHA} -F force=true"))
    );
    assert!(
        i(&format!("git/refs/tags/dev -f sha={SHA}"))
            < i("releases/100 -f tag_name=dev -F draft=false")
    );
    assert_eq!(
        e.r("mock/log").matches("git ls-remote").count(),
        2,
        "late freshness check"
    );
}

#[test]
fn stale_master_does_not_roll_back_dev() {
    let e = Env::new("stale");
    e.seed_old_dev();
    e.w("mock/head", b"3333333333333333333333333333333333333333");
    let before = e.snap();
    let (ok, out) = e.run();
    assert!(ok && out.contains("stale"), "{out}");
    assert!(e.writes().is_empty());
    assert_eq!(e.snap(), before);
}

#[test]
fn master_moving_after_staging_is_not_promoted() {
    let e = Env::new("late");
    e.seed_old_dev();
    e.w(
        "mock/head-after",
        b"3333333333333333333333333333333333333333",
    );
    let before = e.snap();
    let (ok, out) = e.run();
    assert!(ok && out.contains("stale"), "{out}");
    assert_eq!(
        e.snap(),
        before,
        "previous dev intact, staging draft removed"
    );
    assert!(e
        .writes()
        .iter()
        .any(|l| l.contains("-X DELETE repos/k0Iry/cvd-indicators/releases/100")));
}

/// Mid-run failures leave the exact previous remote state (or no dev at all on
/// a first publication) public, never a mixed or unverified set.
#[test]
fn failures_restore_the_exact_previous_state() {
    let cases: [(&str, &str, &str); 6] = [
        ("partial-upload", "fail", "assets\\?name=cvd_indicator_flow"),
        ("bad-staged-bytes", "corrupt-upload", ""),
        ("hide-old", "fail", "releases/50 -F draft=true"),
        (
            "ref-move",
            "fail",
            &*Box::leak(format!("git/refs(/tags/dev)? .*sha={SHA}").into_boxed_str()),
        ),
        (
            "publish-edit",
            "fail",
            "releases/100 -f tag_name=dev -F draft=false",
        ),
        ("bad-readback", "corrupt-public", ""),
    ];
    for first in [false, true] {
        for (name, file, re) in cases {
            if first && name == "hide-old" {
                continue;
            }
            let e = Env::new(&format!("{name}-{first}"));
            if !first {
                e.seed_old_dev();
            }
            let before = e.snap();
            e.w(&format!("mock/{file}"), re.as_bytes());
            let (ok, out) = e.run();
            assert!(!ok, "{name}/{first} must fail: {out}");
            assert!(!out.contains("now publishes"), "{out}");
            assert_eq!(e.snap(), before, "{name}/{first}: {out}");
            e.assert_stable_untouched();
        }
    }
}

#[test]
fn failed_rollback_leaves_nothing_unverified_public() {
    let e = Env::new("rollback-fail");
    e.seed_old_dev();
    e.w("mock/corrupt-public", b"");
    e.w("mock/fail", b"releases/50 -F draft=false\n");
    let (ok, out) = e.run();
    assert!(!ok && out.contains("rollback failed"), "{out}");
    let rel = e.releases();
    assert!(
        !rel.iter().any(|r| r.1 == "dev" && !r.2),
        "no public dev: {rel:?}"
    );
    assert!(
        rel.contains(&("50".into(), "dev-previous-50".into(), true)),
        "{rel:?}"
    );
    e.assert_stable_untouched();
}

/// Every refusal: non-success exit and zero writes.
#[test]
fn invalid_inputs_and_ambiguity_fail_closed_without_writes() {
    let sdk = lock_sdk();
    let foreign_sdk = sdk.replace("k0Iry/cvd-core", "evil/cvd-core");
    type Case = (&'static str, Box<dyn Fn(&Env)>);
    let cases: Vec<Case> = vec![
        (
            "remote-unreachable",
            Box::new(|e| fs::remove_file(e.p("mock/head")).unwrap()),
        ),
        (
            "api-error",
            Box::new(|e| e.w("mock/fail", b"^gh api repos/")),
        ),
        (
            "tampered",
            Box::new(|e| e.w("dist/cvd_indicator_flow.wasm", b"evil")),
        ),
        (
            "missing",
            Box::new(|e| fs::remove_file(e.p("dist/cvd_indicator_orderbook.wasm")).unwrap()),
        ),
        ("unexpected", Box::new(|e| e.w("dist/extra.wasm", b"x"))),
        (
            "unlisted-hash",
            Box::new(|e| {
                let s = e.r("dist/SHA256SUMS");
                let s: String = s
                    .lines()
                    .filter(|l| !l.contains("orderbook"))
                    .map(|l| format!("{l}\n"))
                    .collect();
                e.w("dist/SHA256SUMS", s.as_bytes());
            }),
        ),
        (
            "foreign-commit",
            Box::new(|e| e.dist("dev", OLD, &lock_sdk(), "n")),
        ),
        (
            "stable-mode",
            Box::new(|e| e.dist("v1.2.3", SHA, &lock_sdk(), "n")),
        ),
        (
            "foreign-sdk",
            Box::new(move |e| e.dist("dev", SHA, &foreign_sdk, "n")),
        ),
    ];
    for (name, mutate) in cases {
        let e = Env::new(name);
        mutate(&e);
        let (ok, out) = e.run();
        assert!(!ok, "{name} must fail: {out}");
        assert!(e.writes().is_empty(), "{name}: {:?}", e.writes());
        e.assert_stable_untouched();
    }
}
