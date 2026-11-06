//! Dev tooling, shelling out to `git` and `curl` (no Ruby needed).
//!   cargo xtask corpus-fetch   fetch manifest + goldens at the pinned SHA, download fonts, verify sha256
//!   cargo xtask build-woff2    build the pinned woff2_decompress via fontisan's scripts/build-woff2.sh
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

/// fontist/fontisan branch issue-156/pr162-next (PR #162 ruby plus table-drop sweep fixes). Bumped when goldens land.
const FONTISAN_REPO: &str = "https://github.com/fontist/fontisan.git";
const FONTISAN_SHA: &str = "2b850c4a8cd44e7df28caaf227e69b5bf30619d5";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()
        .unwrap()
}

fn run(cmd: &mut Command) -> Result<(), String> {
    let st = cmd.status().map_err(|e| format!("{cmd:?}: {e}"))?;
    if st.success() {
        Ok(())
    } else {
        Err(format!("{cmd:?} failed: {st}"))
    }
}

fn checkout() -> Result<PathBuf, String> {
    let dir = match std::env::var_os("FONTISAN_CORPUS_DIR") {
        Some(d) => PathBuf::from(d),
        None => root().join(".corpus/fontisan"),
    };
    if !dir.join(".git").exists() {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        run(Command::new("git").current_dir(&dir).args(["init", "-q"]))?;
        run(Command::new("git").current_dir(&dir).args([
            "remote",
            "add",
            "origin",
            FONTISAN_REPO,
        ]))?;
    }
    let g = |a: &[&str]| run(Command::new("git").current_dir(&dir).args(a));
    g(&[
        "sparse-checkout",
        "set",
        "--no-cone",
        "/corpus/",
        "/scripts/build-woff2.sh",
    ])?;
    g(&[
        "fetch",
        "-q",
        "--depth=1",
        "--filter=blob:none",
        "origin",
        FONTISAN_SHA,
    ])?;
    g(&["checkout", "-q", "--detach", FONTISAN_SHA])?;
    Ok(dir)
}

fn sha256(p: &Path) -> Result<String, String> {
    let b = std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(Sha256::digest(&b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect())
}

fn corpus_fetch() -> Result<(), String> {
    // `ci` tier by default; `--large` also fetches the large tier (hundreds of MB).
    let large = std::env::args().any(|a| a == "--large");
    let dir = checkout()?;
    let manifest: Value = serde_json::from_slice(
        &std::fs::read(dir.join("corpus/manifest.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let fonts_dir = dir.join("corpus/fonts");
    std::fs::create_dir_all(&fonts_dir).map_err(|e| e.to_string())?;
    let mut n = 0;
    for f in manifest["fonts"]
        .as_array()
        .ok_or("manifest has no fonts")?
    {
        // Derived fonts are committed in the checkout; only `source` entries are downloaded.
        let Some(url) = f["source"]["url"].as_str() else {
            continue;
        };
        if f["tier"].as_str() == Some("large") && !large {
            continue;
        }
        let (file, want) = (
            f["file"].as_str().ok_or("font without file")?,
            f["sha256"].as_str().ok_or("font without sha256")?,
        );
        let dest = fonts_dir.join(file);
        if !dest.exists() || sha256(&dest)? != want {
            let fetched = if f["source"]["archive_member"].is_string() {
                dest.with_extension("download.zip")
            } else {
                dest.clone()
            };
            run(Command::new("curl")
                .args([
                    "--fail",
                    "--silent",
                    "--show-error",
                    "--location",
                    "--output",
                ])
                .arg(&fetched)
                .arg(url))?;
            if let Some(member) = f["source"]["archive_member"].as_str() {
                let out = std::fs::File::create(&dest).map_err(|e| e.to_string())?;
                run(Command::new("unzip")
                    .args(["-p"])
                    .arg(&fetched)
                    .arg(member)
                    .stdout(out))?;
                std::fs::remove_file(&fetched).map_err(|e| e.to_string())?;
            }
        }
        let got = sha256(&dest)?;
        if got != want {
            return Err(format!("{file}: sha256 {got} != manifest {want}"));
        }
        n += 1;
    }
    println!(
        "corpus-fetch: {n} source fonts verified in {}",
        fonts_dir.display()
    );
    Ok(())
}

fn build_woff2() -> Result<(), String> {
    let dir = checkout()?;
    run(Command::new("bash")
        .current_dir(&dir)
        .arg("scripts/build-woff2.sh"))
}

fn main() {
    let r = match std::env::args().nth(1).as_deref() {
        Some("corpus-fetch") => corpus_fetch(),
        Some("build-woff2") => build_woff2(),
        _ => Err("usage: cargo xtask <corpus-fetch|build-woff2>".into()),
    };
    if let Err(e) = r {
        eprintln!("xtask: {e}");
        std::process::exit(1);
    }
}
