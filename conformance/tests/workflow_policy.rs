//! Local check of the CI trigger/publication policy (no remote CI needed).
//! Simulates which jobs run per event/ref by evaluating the workflow's `if:`
//! expressions, which are restricted to the forms asserted here.
use std::path::Path;

fn workflow() -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    std::fs::read_to_string(root.join(".github/workflows/ci.yml")).unwrap()
}

/// Lines of the top-level `on:` block.
fn triggers(w: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut inside = false;
    for l in w.lines() {
        if l == "on:" {
            inside = true;
            continue;
        }
        if inside {
            if !l.starts_with(' ') {
                break;
            }
            out.push(l.trim().to_string());
        }
    }
    out
}

/// Text of a top-level job block.
fn job<'a>(w: &'a str, name: &str) -> &'a str {
    let start = w
        .find(&format!("\n  {name}:\n"))
        .unwrap_or_else(|| panic!("job {name}"));
    let rest = &w[start + 1..];
    // ends at the next two-space-indented sibling key
    let mut e = rest.len();
    let mut off = 0;
    for l in rest.lines() {
        if off > 0 && l.starts_with("  ") && !l.starts_with("   ") {
            e = off;
            break;
        }
        off += l.len() + 1;
    }
    &rest[..e]
}

const UPSTREAM: &str = "k0Iry/cvd-indicators";
const PUBLISH_IF: &str =
    "github.event_name == 'push' && github.repository == 'k0Iry/cvd-indicators' && startsWith(github.ref, 'refs/tags/v')";
const DEV_IF: &str =
    "github.event_name == 'push' && github.repository == 'k0Iry/cvd-indicators' && github.ref == 'refs/heads/master'";
const UPLOAD_IF: &str = "github.event_name == 'push' && github.repository == 'k0Iry/cvd-indicators' && (startsWith(github.ref, 'refs/tags/v') || github.ref == 'refs/heads/master')";

/// Evaluations of the three asserted `if:` expressions.
fn publishes(event: &str, repo: &str, git_ref: &str) -> bool {
    event == "push" && repo == UPSTREAM && git_ref.starts_with("refs/tags/v")
}
fn publishes_dev(event: &str, repo: &str, git_ref: &str) -> bool {
    event == "push" && repo == UPSTREAM && git_ref == "refs/heads/master"
}
fn uploads(event: &str, repo: &str, git_ref: &str) -> bool {
    event == "push"
        && repo == UPSTREAM
        && (git_ref.starts_with("refs/tags/v") || git_ref == "refs/heads/master")
}

/// (event, repository, ref, stable release, dev refresh)
const MATRIX: [(&str, &str, &str, bool, bool); 9] = [
    ("push", UPSTREAM, "refs/tags/v1.2.3", true, false),
    ("push", UPSTREAM, "refs/heads/master", false, true),
    ("push", UPSTREAM, "refs/heads/feature", false, false),
    ("push", UPSTREAM, "refs/heads/master-x", false, false),
    (
        "push",
        "fork/cvd-indicators",
        "refs/heads/master",
        false,
        false,
    ),
    (
        "push",
        "fork/cvd-indicators",
        "refs/tags/v1.2.3",
        false,
        false,
    ),
    ("pull_request", UPSTREAM, "refs/pull/7/merge", false, false),
    (
        "workflow_dispatch",
        UPSTREAM,
        "refs/heads/master",
        false,
        false,
    ),
    (
        "workflow_dispatch",
        UPSTREAM,
        "refs/tags/v1.2.3",
        false,
        false,
    ),
];

#[test]
fn triggers_include_push_pr_and_manual() {
    let t = triggers(&workflow());
    for k in ["push:", "pull_request:", "workflow_dispatch:"] {
        assert!(t.iter().any(|l| l == k), "missing trigger {k}: {t:?}");
    }
    assert!(
        t.iter().any(|l| l == "tags: ['v[0-9]+.[0-9]+.[0-9]+']"),
        "{t:?}"
    );
    assert!(
        !t.iter()
            .any(|l| l.starts_with("pull_request_target") || l.starts_with("release:")),
        "{t:?}"
    );
}

#[test]
fn only_version_tag_pushes_publish() {
    let w = workflow();
    let release = job(&w, "release");
    assert!(
        release.contains(&format!("if: {PUBLISH_IF}")),
        "release job gate"
    );
    assert!(release.contains("needs: gate"));
    // trusted ref: strict tag regex at runtime + ancestry on the actual default branch
    assert!(release.contains(r#"[[ "$GITHUB_EVENT_NAME" == push && "$GITHUB_REF" =~ ^refs/tags/v[0-9]+\.[0-9]+\.[0-9]+$ ]]"#));
    assert!(release.contains("git merge-base --is-ancestor \"$GITHUB_SHA\" origin/master"));
    assert!(
        !w.contains("origin/main") && !w.contains("refs/heads/main"),
        "default branch is master"
    );
    assert!(release.contains("persist-credentials: false"));
    // payload provenance: this tag's commit, from the gate's own checkout
    assert!(
        release.contains(r#"--arg c "$GITHUB_SHA" '.release == $t and .source == {commit: $c}"#)
    );
    // a formal release is created once; `gh release create` refuses an existing one
    assert!(
        !release.contains("--clobber")
            && !release.contains("release upload")
            && !release.contains("release edit")
    );
    for (ev, repo, r, want, _) in MATRIX {
        assert_eq!(publishes(ev, repo, r), want, "{ev} {repo} {r}");
    }
}

#[test]
fn only_upstream_master_pushes_refresh_dev() {
    let w = workflow();
    let dev = job(&w, "publish-dev");
    assert!(dev.contains(&format!("if: {DEV_IF}")), "publish-dev gate");
    assert!(dev.contains("needs: gate"));
    // serialized, never cancelling a running publication
    assert!(
        dev.contains("concurrency:\n      group: publish-dev\n      cancel-in-progress: false\n")
    );
    assert!(dev.contains("persist-credentials: false"));
    assert!(dev.contains("name: public-dist") && dev.contains("path: dist"));
    assert!(dev.contains("GH_TOKEN: ${{ github.token }}"));
    assert!(dev.contains("run: scripts/publish-dev.sh dist"));
    // reuses the gate dist: no build/package in the publisher
    for no in ["cargo", "public-gate", "rustup", "secrets."] {
        assert!(!dev.contains(no), "publish-dev must not contain {no}");
    }
    // gate records release `dev` exactly for master pushes
    assert!(w.contains("github.event_name == 'push' && github.ref == 'refs/heads/master' && 'dev'"));
    // the `dev` tag never matches the tag trigger (no second build)
    assert!(!triggers(&w).iter().any(|l| l.contains("dev")));
    for (ev, repo, r, _, want) in MATRIX {
        assert_eq!(publishes_dev(ev, repo, r), want, "{ev} {repo} {r}");
        assert!(!(publishes(ev, repo, r) && publishes_dev(ev, repo, r)));
    }
}

#[test]
fn publish_dev_script_writes_only_the_dev_release() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let s = std::fs::read_to_string(root.join("scripts/publish-dev.sh")).unwrap();
    assert!(s.contains("\nTAG=dev\n"));
    assert!(!s.contains("gh release "), "only id-bound gh api writes");
    // every write targets this run's draft, the previous dev release id
    // (read from releases/tags/$TAG), or refs/tags/$TAG
    let ok = [
        "\"$R/releases\" -f tag_name=\"$STAGE\"",
        "\"$R/releases/$NEW_ID\"",
        "\"$R/releases/$OLD_ID\"",
        "\"https://uploads.github.com/$R/releases/$NEW_ID/assets?name=$f\"",
        "\"$R/git/refs/tags/$TAG\"",
        "\"$R/git/refs\" -f ref=\"refs/tags/$TAG\"",
    ];
    let joined = s.replace("\\\n", " ");
    for l in joined.lines().filter(|l| l.contains("gh api -X")) {
        assert!(ok.iter().any(|o| l.contains(o)), "unbound write: {l}");
        if l.contains("draft=false") {
            assert!(
                l.contains("-f make_latest=false") && l.contains("-F prerelease=true"),
                "{l}"
            );
        }
    }
    assert!(s.contains("OLD_ID=$(jq -er .id <<<\"$old\")"));
    assert_eq!(
        s.matches("master_is_us || rc=$?").count(),
        2,
        "initial + late freshness"
    );
    assert!(!s.contains("secrets.") && !s.contains("set -x"));
    // stable: manifest SDK must equal this checkout's complete Cargo.lock pin
    let w = workflow();
    let rel = job(&w, "release");
    assert!(rel.contains(".sdk.source == $s' --arg s \"$SDK\" manifest.json"));
    assert!(rel.contains("Cargo.lock") && !rel.contains("startswith"));
}

#[test]
fn permissions_and_assets_are_bounded() {
    let w = workflow();
    assert!(
        w.contains("\npermissions:\n  contents: read\n"),
        "top-level read-only"
    );
    let gate = job(&w, "gate");
    assert!(
        !gate.contains("contents: write"),
        "gate job must stay read-only"
    );
    assert!(
        gate.contains(&format!("if: {UPLOAD_IF}")),
        "dist upload only for publishing refs"
    );
    for (ev, repo, r, stable, dev) in MATRIX {
        assert_eq!(uploads(ev, repo, r), stable || dev, "{ev} {repo} {r}");
    }
    let release = job(&w, "release");
    assert_eq!(w.matches("contents: write").count(), 2);
    assert!(release.contains("contents: write"));
    assert!(job(&w, "publish-dev").contains("contents: write"));
    // explicit allowlist, never a directory/glob
    for a in [
        "cvd_indicator_cvd.wasm",
        "cvd_indicator_flow.wasm",
        "cvd_indicator_example_range.wasm",
        "manifest.json",
        "SHA256SUMS",
    ] {
        assert!(release.contains(&format!("dist/{a}")), "{a}");
    }
    assert!(
        !w.contains("target/wasm32") || !w.contains("path: target"),
        "never upload target"
    );
    assert!(
        !w.contains("*.wasm") && !w.contains("dist/*"),
        "no globbed publication"
    );
    assert!(!w.contains("cvd-indicators-private"), "no private repo");
}

/// The exact public module set (conformance `PUBLIC`, which drives packaging)
/// must agree with all three explicit release allowlists: gate upload,
/// release download verification, and `gh release create` assets.
#[test]
fn release_allowlists_match_public_package_set_exactly() {
    use std::collections::BTreeSet;
    let w = workflow();
    let mut want: BTreeSet<String> = cvd_indicators_conformance::PUBLIC
        .iter()
        .map(|(_, wasm, _)| wasm.to_string())
        .collect();
    assert!(
        want.contains("cvd_indicator_orderbook.wasm"),
        "orderbook is public"
    );
    want.insert("manifest.json".into());
    want.insert("SHA256SUMS".into());
    let names = |text: &str, prefix: &str| -> BTreeSet<String> {
        text.split(|c: char| c.is_whitespace() || c == '"' || c == '\\')
            .filter_map(|t| t.find(prefix).map(|i| &t[i + prefix.len()..]))
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .collect()
    };
    // 1. upload (gate job)
    let gate = job(&w, "gate");
    let up = &gate[gate.find("name: public-dist").expect("upload step")..];
    let up = &up[..up.find("\n\n").unwrap_or(up.len())];
    assert_eq!(names(up, "}}/gate/dist/"), want, "upload allowlist");
    let release = job(&w, "release");
    // 2. download verification: exact sorted listing
    let line = release
        .lines()
        .find(|l| l.contains("test \"$(ls | sort"))
        .expect("exact-set listing check");
    let listed = line
        .rsplit("= \"")
        .next()
        .unwrap()
        .trim_end_matches('"')
        .trim();
    let listed: Vec<&str> = listed.split_whitespace().collect();
    let mut sorted = listed.clone();
    sorted.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
    assert_eq!(listed, sorted, "ls | sort order");
    assert_eq!(
        listed
            .iter()
            .map(|s| s.to_string())
            .collect::<BTreeSet<_>>(),
        want,
        "verify allowlist"
    );
    // 3. published assets
    let publish = &release[release.find("gh release create").expect("publish")..];
    assert_eq!(names(publish, "dist/"), want, "publish allowlist");
}
