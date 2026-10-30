//! Packaging output safety: owned disposable temp dirs and marker files only.
use cvd_indicators_conformance::dist::{install, Files, OWNER_MARKER};
use std::fs;
use std::path::{Path, PathBuf};

const ALLOWED: [&str; 2] = ["a.wasm", "manifest.json"];

fn tmp(tag: &str) -> PathBuf {
    let base = std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let d = base.join(format!("cvd-dist-safety-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d); // our own uniquely named disposable dir
    fs::create_dir_all(&d).unwrap();
    d
}
fn files(v: &str) -> Files {
    vec![
        ("a.wasm".into(), format!("wasm-{v}").into_bytes()),
        (
            "manifest.json".into(),
            format!("{{\"v\":\"{v}\"}}").into_bytes(),
        ),
    ]
}
fn snapshot(d: &Path) -> Vec<(String, Vec<u8>)> {
    let mut v: Vec<_> = fs::read_dir(d)
        .unwrap()
        .map(|e| {
            let p = e.unwrap().path();
            let n = p.file_name().unwrap().to_string_lossy().into_owned();
            let b = if p.is_dir() {
                snapshot(&p)
                    .into_iter()
                    .flat_map(|(n, b)| [n.into_bytes(), b].concat())
                    .collect()
            } else {
                fs::read(&p).unwrap()
            };
            (n, b)
        })
        .collect();
    v.sort();
    v
}
fn no_leftovers(parent: &Path) {
    for e in fs::read_dir(parent).unwrap() {
        let n = e.unwrap().file_name().to_string_lossy().into_owned();
        assert!(
            !n.contains(".staging-") && !n.contains(".old-"),
            "leftover {n}"
        );
    }
}

#[test]
fn fresh_and_repeated_packaging() {
    let root = tmp("fresh");
    let out = root.join("dist");
    install(&out, &ALLOWED, || Ok(files("1"))).unwrap();
    assert_eq!(fs::read(out.join("a.wasm")).unwrap(), b"wasm-1");
    assert!(out.join(OWNER_MARKER).is_file());
    install(&out, &ALLOWED, || Ok(files("2"))).unwrap();
    assert_eq!(fs::read(out.join("a.wasm")).unwrap(), b"wasm-2");
    assert_eq!(fs::read_dir(&out).unwrap().count(), 3);
    // existing empty dir is fine too
    let empty = root.join("empty");
    fs::create_dir(&empty).unwrap();
    install(&empty, &ALLOWED, || Ok(files("3"))).unwrap();
    no_leftovers(&root);
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn refuses_unrelated_destinations_untouched() {
    let root = tmp("unrelated");
    type Setup = Box<dyn Fn(&Path)>;
    let cases: Vec<(&str, Setup)> = vec![
        (
            "marker-file",
            Box::new(|d: &Path| fs::write(d.join("OWNED_MARKER.txt"), b"keep").unwrap()),
        ),
        (
            "allowlisted-name-but-unowned",
            Box::new(|d: &Path| fs::write(d.join("a.wasm"), b"user bytes").unwrap()),
        ),
        (
            "subdir",
            Box::new(|d: &Path| {
                fs::write(d.join(OWNER_MARKER), b"x").unwrap();
                fs::create_dir(d.join("sub")).unwrap();
                fs::write(d.join("sub/m"), b"keep").unwrap();
            }),
        ),
        (
            "owned-plus-extra",
            Box::new(|d: &Path| {
                fs::write(d.join(OWNER_MARKER), b"x").unwrap();
                fs::write(d.join("a.wasm"), b"old").unwrap();
                fs::write(d.join("notes.txt"), b"keep").unwrap();
            }),
        ),
    ];
    for (name, setup) in cases {
        let d = root.join(name);
        fs::create_dir(&d).unwrap();
        setup(&d);
        let before = snapshot(&d);
        let mut produced = false;
        let err = install(&d, &ALLOWED, || {
            produced = true;
            Ok(files("new"))
        })
        .unwrap_err();
        assert!(err.contains("refusing"), "{name}: {err}");
        assert!(!produced, "{name}: must refuse before building");
        assert_eq!(snapshot(&d), before, "{name}: bytes preserved");
    }
    let f = root.join("plain-file");
    fs::write(&f, b"keep").unwrap();
    assert!(install(&f, &ALLOWED, || Ok(files("x"))).is_err());
    assert_eq!(fs::read(&f).unwrap(), b"keep");
    no_leftovers(&root);
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn injected_failure_preserves_existing_dist() {
    let root = tmp("fail");
    let out = root.join("dist");
    install(&out, &ALLOWED, || Ok(files("good"))).unwrap();
    let before = snapshot(&out);
    let err = install(&out, &ALLOWED, || Err("injected validation failure".into())).unwrap_err();
    assert!(err.contains("injected"));
    assert_eq!(snapshot(&out), before);
    let err = install(&out, &ALLOWED, || Ok(vec![("evil.bin".into(), vec![1])])).unwrap_err();
    assert!(err.contains("allowlist"), "{err}");
    assert_eq!(snapshot(&out), before);
    no_leftovers(&root);
    fs::remove_dir_all(&root).unwrap();
}
