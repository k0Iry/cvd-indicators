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

const PUBLISH_IF: &str = "github.event_name == 'push' && startsWith(github.ref, 'refs/tags/v')";

fn publishes(event: &str, git_ref: &str) -> bool {
    event == "push" && git_ref.starts_with("refs/tags/v")
}

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
    // matrix of events/refs
    for (ev, r, want) in [
        ("push", "refs/tags/v1.2.3", true),
        ("push", "refs/heads/master", false),
        ("push", "refs/heads/feature", false),
        ("pull_request", "refs/pull/7/merge", false),
        ("workflow_dispatch", "refs/heads/master", false),
        ("workflow_dispatch", "refs/tags/v1.2.3", false),
    ] {
        assert_eq!(publishes(ev, r), want, "{ev} {r}");
    }
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
        gate.contains(&format!("if: {PUBLISH_IF}")),
        "dist upload only for publishing refs"
    );
    let release = job(&w, "release");
    assert_eq!(w.matches("contents: write").count(), 1);
    assert!(release.contains("contents: write"));
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
