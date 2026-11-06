//! Golden harness. Goldens live in fontist/fontisan at a pinned SHA and are
//! fetched by `cargo xtask corpus-fetch` into `.corpus/fontisan` (or the
//! directory in `FONTISAN_CORPUS_DIR`). Every case is PENDING until the card
//! that owns it adds its id to `ENABLED`; with no goldens present the harness
//! passes and says so.
use std::fs;
use std::path::{Path, PathBuf};

/// Golden case ids (path relative to `corpus/goldens`) whose engine support
/// has landed. Each later card appends here. Empty in 1a.
const ENABLED: &[&str] = &[];

fn corpus_dir() -> PathBuf {
    match std::env::var_os("FONTISAN_CORPUS_DIR") {
        Some(d) => PathBuf::from(d),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.corpus/fontisan"),
    }
}

fn collect(dir: &Path, root: &Path, out: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, root, out);
        } else if p.extension().is_some_and(|x| x == "json") {
            out.push(
                p.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
}

fn goldens() -> Vec<String> {
    let root = corpus_dir().join("corpus/goldens");
    let mut v = Vec::new();
    collect(&root, &root, &mut v);
    v.sort();
    v
}

#[test]
fn every_golden_case_is_listed_and_pending() {
    let cases = goldens();
    if cases.is_empty() {
        println!("0 goldens found under {}", corpus_dir().display());
        return;
    }
    let pending: Vec<_> = cases
        .iter()
        .filter(|c| !ENABLED.contains(&c.as_str()))
        .collect();
    for c in &pending {
        println!("PENDING {c}");
    }
    println!(
        "{} goldens found, {} pending, {} enabled",
        cases.len(),
        pending.len(),
        cases.len() - pending.len()
    );
    // An enabled id with no golden file is a typo, not a pass.
    for id in ENABLED {
        assert!(
            cases.iter().any(|c| c == id),
            "enabled golden {id} not found"
        );
    }
}
