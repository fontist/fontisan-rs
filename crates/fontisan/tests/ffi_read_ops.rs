//! 1g-abi: the read-side C ABI (`read`, `audit`, `scan`, `audit_dir`) driven through
//! `fontisan_call` against the read, audit, scan and 0e malformed goldens, plus a
//! threaded run. Payloads are packed JSON rows (see `ffi::read_ops`).
mod common;
#[path = "common/goldens.rs"]
mod goldens;

use common::*;
use fontisan::ffi::*;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

enum Outcome {
    Rows(Vec<Value>),
    Err(i32, String),
}

fn call(op: u32, flags: u32, input: &[u8]) -> Outcome {
    let req = FontisanRequest {
        header: FontisanHeader::current::<FontisanRequest>(),
        op,
        flags,
        input: input.as_ptr(),
        input_len: input.len(),
    };
    let mut out = std::ptr::null_mut();
    let status = unsafe { fontisan_call(&req, &mut out) };
    let r = unsafe { &*out };
    let outcome = if status == 0 {
        let p = unsafe { std::slice::from_raw_parts(r.payload, r.payload_len) };
        Outcome::Rows(unpack(p))
    } else {
        let m = unsafe { std::slice::from_raw_parts(r.error.message, r.error.message_len) };
        Outcome::Err(status, String::from_utf8(m.to_vec()).unwrap())
    };
    unsafe { fontisan_result_free(out) };
    outcome
}

fn unpack(p: &[u8]) -> Vec<Value> {
    let n = u32::from_be_bytes(p[..4].try_into().unwrap()) as usize;
    let mut at = 4;
    let rows: Vec<Value> = (0..n)
        .map(|_| {
            let len = u32::from_be_bytes(p[at..at + 4].try_into().unwrap()) as usize;
            let v = serde_json::from_slice(&p[at + 4..at + 4 + len]).unwrap();
            at += 4 + len;
            v
        })
        .collect();
    assert_eq!(at, p.len(), "payload has trailing bytes");
    rows
}

fn rows(o: Outcome, what: &str) -> Vec<Value> {
    match o {
        Outcome::Rows(r) => r,
        Outcome::Err(s, m) => panic!("{what}: status {s}: {m}"),
    }
}

fn norm(s: &str) -> String {
    let mut out = String::new();
    let mut in_digits = false;
    for c in s.chars() {
        if c.is_ascii_digit() {
            if !in_digits {
                out.push_str("<N>");
            }
            in_digits = true;
        } else {
            in_digits = false;
            out.push(c);
        }
    }
    out
}

fn read_flags(key: &str) -> u32 {
    let (mode, lazy) = key.split_once('/').unwrap();
    (if mode == "metadata" {
        READ_FLAG_METADATA
    } else {
        0
    }) | (if lazy == "lazy" { READ_FLAG_LAZY } else { 0 })
}

fn audit_input(name: &str, bytes: &[u8]) -> Vec<u8> {
    let mut v = (name.len() as u32).to_be_bytes().to_vec();
    v.extend_from_slice(name.as_bytes());
    v.extend_from_slice(bytes);
    v
}

/// The error surface of an ABI error result: `{class, message, causes}`.
fn err_value(status: i32, msg: &str, normalise: bool) -> Value {
    assert_eq!(status, FontisanStatus::ReadFailed as i32, "{msg}");
    let mut v: Value = serde_json::from_str(msg).unwrap();
    if !normalise {
        return v;
    }
    v["message"] = json!(norm(v["message"].as_str().unwrap()));
    for c in v["causes"].as_array_mut().unwrap() {
        c["message"] = json!(norm(c["message"].as_str().unwrap()));
    }
    v
}

#[test]
fn read_matches_goldens_through_the_abi() {
    let Some(corpus) = corpus() else { return };
    let Some(root) = goldens::goldens_root() else {
        return;
    };
    let (mut ok, mut errors, mut unsupported) = (0, 0, 0);
    for case in goldens::cases(&corpus, &root) {
        for key in ["full/lazy", "full/eager", "metadata/lazy", "metadata/eager"] {
            if case.swallow && key != "full/eager" {
                continue;
            }
            let want = goldens::read_entry(&case.golden, key);
            let got = call(OP_READ, read_flags(key), &case.bytes);
            let what = format!("{} {key}", case.id);
            if let Some(faces) = want["ok"].as_array() {
                if matches!(got, Outcome::Err(s, _) if s == FontisanStatus::Unsupported as i32) {
                    unsupported += 1; // dfont: the golden is ok, Ruby serves it
                    continue;
                }
                let got = rows(got, &what);
                assert_eq!(got.len(), faces.len(), "{what}: face count");
                // The golden dedups an eager entry as `same_as` lazy, ignoring the echoed `lazy`.
                let deduped = case.golden["read"][key]["same_as"].is_string();
                for (i, (g, w)) in got.iter().zip(faces).enumerate() {
                    let (mut g, w) = (g.clone(), w.clone());
                    if deduped {
                        assert_eq!(
                            g["lazy"],
                            json!(read_flags(key) & READ_FLAG_LAZY != 0),
                            "{what}"
                        );
                        g["lazy"] = w["lazy"].clone();
                    }
                    let (g, w) = (&g, &w);
                    let diff: Vec<_> = w
                        .as_object()
                        .unwrap()
                        .keys()
                        .filter(|k| g[k.as_str()] != w[k.as_str()])
                        .collect();
                    assert!(
                        diff.is_empty() && g == w,
                        "{what}[{i}]: differing keys {diff:?}: {} vs {}",
                        diff.first().map_or(String::new(), |k| g[k.as_str()]
                            .to_string()
                            .chars()
                            .take(300)
                            .collect()),
                        diff.first().map_or(String::new(), |k| w[k.as_str()]
                            .to_string()
                            .chars()
                            .take(300)
                            .collect())
                    );
                    ok += 1;
                }
            } else {
                let Outcome::Err(status, msg) = got else {
                    panic!("{what}: expected an error")
                };
                if status == FontisanStatus::Unsupported as i32 {
                    unsupported += 1;
                    continue; // dfont / Type 1: Ruby serves them
                }
                let e = err_value(status, &msg, false);
                assert_eq!(e["class"], want["error"]["class"], "{what}");
                assert_eq!(e["message"], want["error"]["message"], "{what}");
                errors += 1;
            }
        }
    }
    eprintln!("abi read: {ok} faces equal, {errors} errors equal, {unsupported} unsupported");
    assert!(ok >= 150 && unsupported >= 4, "{ok}/{unsupported}");
}

#[test]
fn audit_matches_goldens_through_the_abi() {
    let Some(corpus) = corpus() else { return };
    let Some(root) = goldens::goldens_root() else {
        return;
    };
    let (mut ok, mut errors, mut unsupported) = (0, 0, 0);
    for case in goldens::cases(&corpus, &root) {
        for (profile, flags) in [("default", 0), ("validate", AUDIT_FLAG_VALIDATE)] {
            let want = &case.golden["audit"][profile];
            let got = call(OP_AUDIT, flags, &audit_input(&case.file, &case.bytes));
            let what = format!("{} {profile}", case.id);
            if let Some(faces) = want["ok"].as_array() {
                if matches!(got, Outcome::Err(s, _) if s == FontisanStatus::Unsupported as i32) {
                    unsupported += 1;
                    continue;
                }
                let got = rows(got, &what);
                assert_eq!(got.len(), faces.len(), "{what}: count");
                for (i, (g, w)) in got.iter().zip(faces).enumerate() {
                    assert_eq!(g, w, "{what}[{i}]");
                    ok += 1;
                }
            } else {
                let Outcome::Err(status, msg) = got else {
                    panic!("{what}: expected an error")
                };
                if status == FontisanStatus::Unsupported as i32 {
                    unsupported += 1;
                    continue;
                }
                let e = err_value(status, &msg, false);
                assert_eq!(e["class"], want["error"]["class"], "{what}");
                assert_eq!(e["message"], want["error"]["message"], "{what}");
                errors += 1;
            }
        }
    }
    eprintln!("abi audit: {ok} reports equal, {errors} errors equal, {unsupported} unsupported");
    assert!(ok >= 60, "{ok}");
}

fn scan_goldens(corpus: &Path) -> Vec<(String, String, Vec<Value>)> {
    let dir = corpus_dir().join("corpus/goldens/scan");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    files
        .iter()
        .map(|p| {
            let g: Value = serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap();
            let file = g["file"].as_str().unwrap().to_string();
            assert!(font_path(corpus, &file).exists());
            (
                g["id"].as_str().unwrap().to_string(),
                file,
                g["rows"].as_array().unwrap().clone(),
            )
        })
        .collect()
}

fn assert_scan_row(got: &Value, want: &Value, what: &str) {
    let mut g = got.as_object().unwrap().clone();
    assert_eq!(g.remove("status").unwrap(), "ok", "{what}");
    let path = g.remove("path").unwrap();
    let w = want.as_object().unwrap();
    for (k, wv) in w {
        if k == "path" {
            continue;
        }
        assert_eq!(g.get(k), Some(wv), "{what}.{k}");
    }
    assert_eq!(
        g.len(),
        w.len() - usize::from(w.contains_key("path")),
        "{what}: fields"
    );
    assert!(
        path.as_str()
            .unwrap()
            .ends_with(w["path"].as_str().unwrap_or("")),
        "{what}: path"
    );
}

#[test]
fn scan_one_file_matches_scan_goldens_through_the_abi() {
    let Some(corpus) = corpus() else { return };
    if !corpus_dir().join("corpus/goldens/scan").is_dir() {
        return;
    }
    let (mut matched, mut unsupported) = (0, 0);
    for (id, file, want) in scan_goldens(&corpus) {
        let path = font_path(&corpus, &file);
        let got = rows(call(OP_SCAN, 0, path.to_str().unwrap().as_bytes()), &id);
        if got[0]["status"] == "unsupported" {
            assert_eq!(got.len(), 1, "{id}");
            unsupported += 1;
            continue;
        }
        assert_eq!(got.len(), want.len(), "{id}: rows");
        for (n, (g, w)) in got.iter().zip(&want).enumerate() {
            assert_scan_row(g, w, &format!("{id}[{n}]"));
            matched += 1;
        }
    }
    eprintln!("abi scan: {matched} rows equal, {unsupported} unsupported");
    assert!(matched >= 40 && unsupported >= 2, "{matched}/{unsupported}");
}

fn mixed_dir(corpus: &Path) -> (PathBuf, Vec<String>) {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("fontisan-1g-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    let mut files = Vec::new();
    for (name, sub) in [
        ("TestTTF.ttf", ""),
        ("TestTTC.ttc", ""),
        ("TestDFONT.dfont", ""),
        ("TestT1-Regular.pfb", "sub/"),
        ("TestOTF.otf", "sub/"),
    ] {
        std::fs::copy(font_path(corpus, name), dir.join(sub).join(name)).unwrap();
        files.push(name.to_string());
    }
    std::fs::write(dir.join("README.txt"), b"not a font").unwrap();
    // a malformed font in the same directory is a row, not an abort
    let mut bad = std::fs::read(font_path(corpus, "TestTTF.ttf")).unwrap();
    bad.truncate(100);
    std::fs::write(dir.join("broken.ttf"), bad).unwrap();
    files.push("broken.ttf".into());
    (dir, files)
}

#[test]
fn scan_and_audit_dir_serve_a_mixed_directory_in_one_call() {
    let Some(corpus) = corpus() else { return };
    let (dir, _) = mixed_dir(&corpus);
    let d = dir.to_str().unwrap().as_bytes();
    let scan = rows(call(OP_SCAN, 0, d), "scan dir");
    // sorted by path; TTC is one row per member; unsupported rows for dfont and pfb
    let paths: Vec<_> = scan
        .iter()
        .map(|r| r["path"].as_str().unwrap().to_string())
        .collect();
    let mut sorted = paths.clone();
    sorted.sort();
    assert_eq!(paths, sorted);
    assert!(!paths.iter().any(|p| p.ends_with("README.txt")));
    let status = |name: &str| -> Vec<&str> {
        scan.iter()
            .filter(|r| r["path"].as_str().unwrap().ends_with(name))
            .map(|r| r["status"].as_str().unwrap())
            .collect()
    };
    assert_eq!(status("TestDFONT.dfont"), ["unsupported"], "{paths:?}");
    assert_eq!(status("TestT1-Regular.pfb"), ["unsupported"]);
    assert_eq!(status("TestTTF.ttf"), ["ok"]);
    assert_eq!(status("TestTTC.ttc"), ["ok", "ok"]);
    let broken = scan
        .iter()
        .find(|r| r["path"].as_str().unwrap().ends_with("broken.ttf"))
        .unwrap();
    assert_eq!(broken["status"], "ok");
    assert!(!broken["errors"]["font_file"].is_null());

    for (flags, validated) in [(0, false), (AUDIT_FLAG_VALIDATE, true)] {
        let audit = rows(call(OP_AUDIT_DIR, flags, d), "audit_dir");
        let by = |name: &str| {
            audit
                .iter()
                .find(|r| r["path"].as_str().unwrap().ends_with(name))
                .unwrap()
        };
        assert_eq!(audit.len(), 6);
        assert_eq!(by("TestDFONT.dfont")["status"], "unsupported");
        assert_eq!(by("TestT1-Regular.pfb")["status"], "unsupported");
        assert_eq!(by("TestTTC.ttc")["reports"].as_array().unwrap().len(), 2);
        let ttf = &by("TestTTF.ttf")["reports"][0];
        assert_eq!(ttf["source_file"], "TestTTF.ttf");
        assert_eq!(ttf.get("validation_summary").is_some(), validated);
        let b = by("broken.ttf");
        assert_eq!(b["status"], "error");
        assert_eq!(b["error"]["class"], "Fontisan::Error");
        assert_eq!(b["error"]["causes"][0]["class"], "RuntimeError");
        // the single-file audit gives the same report
        let bytes = std::fs::read(dir.join("TestTTF.ttf")).unwrap();
        let single = rows(
            call(OP_AUDIT, flags, &audit_input("TestTTF.ttf", &bytes)),
            "audit",
        );
        assert_eq!(single[0], *ttf);
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn malformed_goldens_match_through_the_abi() {
    let Some(corpus) = corpus() else { return };
    let fetched = corpus_dir().join("corpus/goldens/malformed");
    let dir = if fetched.is_dir() {
        fetched
    } else {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/malformed")
    };
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    assert_eq!(files.len(), 8);
    let (mut reads, mut audits, mut scans) = (0, 0, 0);
    for path in files {
        let g: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let id = g["id"].as_str().unwrap();
        let file = font_path(&corpus, g["file"].as_str().unwrap());
        let bytes = std::fs::read(&file).unwrap();
        for (combo, flags) in [
            ("full/lazy=false", 0),
            ("full/lazy=true", READ_FLAG_LAZY),
            ("metadata/lazy=false", READ_FLAG_METADATA),
            ("metadata/lazy=true", READ_FLAG_METADATA | READ_FLAG_LAZY),
        ] {
            let want = &g["combos"][combo]["ops"]["read"];
            let got = call(OP_READ, flags, &bytes);
            let what = format!("{id} {combo}");
            if want["outcome"] == "ok" {
                rows(got, &what);
            } else {
                let Outcome::Err(status, msg) = got else {
                    panic!("{what}: expected an error")
                };
                let e = err_value(status, &msg, true);
                assert_eq!(e["class"], want["error_class"], "{what}");
                assert_eq!(e["message"], want["message"], "{what}");
                assert_eq!(e["causes"], want["causes"], "{what}");
            }
            reads += 1;
        }
        let want = &g["audit"];
        let got = call(
            OP_AUDIT,
            0,
            &audit_input(g["file"].as_str().unwrap(), &bytes),
        );
        if want["outcome"] == "ok" {
            rows(got, id);
        } else {
            let Outcome::Err(status, msg) = got else {
                panic!("{id}: audit expected an error")
            };
            let e = err_value(status, &msg, true);
            assert_eq!(e["class"], want["error_class"], "{id} audit");
            assert_eq!(e["message"], want["message"], "{id} audit");
            assert_eq!(e["causes"], want["causes"], "{id} audit");
        }
        audits += 1;
        let want = &g["scan"]["summary"][0];
        let got = &rows(call(OP_SCAN, 0, file.to_str().unwrap().as_bytes()), id)[0];
        assert_eq!(got["format"], want["format"], "{id} scan format");
        assert_eq!(got["num_faces"], want["num_faces"], "{id} scan faces");
        for slot in ["font_file", "collection_file"] {
            let (g, w) = (&got["errors"][slot], &want["errors"][slot]);
            if w.is_null() {
                assert!(g.is_null(), "{id} scan {slot}");
                continue;
            }
            assert_eq!(g["class"], w["class"], "{id} scan {slot}");
            assert_eq!(
                json!(norm(g["message"].as_str().unwrap())),
                w["message"],
                "{id} scan {slot}"
            );
            assert_eq!(
                g["cause"]["class"], w["cause"]["class"],
                "{id} scan {slot} cause"
            );
        }
        scans += 1;
    }
    eprintln!("abi malformed: {reads} read combos, {audits} audits, {scans} scans equal");
}

#[test]
fn concurrent_callers_get_the_same_answers() {
    let Some(corpus) = corpus() else { return };
    let names = [
        "TestTTF.ttf",
        "TestOTF.otf",
        "TestTTC.ttc",
        "SourceSans3-Regular.otf",
    ];
    let inputs: Vec<(String, Vec<u8>)> = names
        .iter()
        .map(|n| (n.to_string(), std::fs::read(font_path(&corpus, n)).unwrap()))
        .collect();
    let (dir, _) = mixed_dir(&corpus);
    let dir_bytes = dir.to_str().unwrap().as_bytes().to_vec();
    let once = |inputs: &[(String, Vec<u8>)]| -> Vec<Vec<Value>> {
        let mut all = Vec::new();
        for (n, b) in inputs {
            all.push(rows(call(OP_READ, 0, b), n));
            all.push(rows(
                call(OP_AUDIT, AUDIT_FLAG_VALIDATE, &audit_input(n, b)),
                n,
            ));
        }
        all.push(rows(call(OP_SCAN, 0, &dir_bytes), "scan"));
        all.push(rows(call(OP_AUDIT_DIR, 0, &dir_bytes), "audit_dir"));
        all
    };
    let baseline = once(&inputs);
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..8)
            .map(|_| s.spawn(|| (0..3).map(|_| once(&inputs)).collect::<Vec<_>>()))
            .collect();
        for h in handles {
            for run in h.join().unwrap() {
                assert_eq!(run, baseline);
            }
        }
    });
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn bad_arguments_are_errors_not_panics() {
    // too short for the source-name prefix
    let Outcome::Err(s, _) = call(OP_AUDIT, 0, &[0, 0]) else {
        panic!()
    };
    assert_eq!(s, FontisanStatus::InvalidArgument as i32);
    // a name length past the end of the input
    let Outcome::Err(s, _) = call(OP_AUDIT, 0, &[0, 0, 1, 0, b'x']) else {
        panic!()
    };
    assert_eq!(s, FontisanStatus::InvalidArgument as i32);
    let Outcome::Err(s, _) = call(OP_SCAN, 0, &[0xff, 0xfe]) else {
        panic!()
    };
    assert_eq!(s, FontisanStatus::InvalidArgument as i32);
    // an unreadable path is a scan row (Errno::ENOENT), like Ruby
    let r = rows(call(OP_SCAN, 0, b"/nonexistent/none.ttf"), "scan missing");
    assert!(
        r[0]["errors"]["font_file"]["message"]
            .as_str()
            .unwrap()
            .contains("Errno::ENOENT")
    );
    // empty and garbage fonts: read fails with a Ruby-shaped error
    let Outcome::Err(s, m) = call(OP_READ, 0, b"") else {
        panic!()
    };
    assert_eq!(s, FontisanStatus::ReadFailed as i32, "{m}");
    let Outcome::Err(s, _) = call(OP_READ, 0, &[0xAB; 64]) else {
        panic!()
    };
    assert_eq!(s, FontisanStatus::ReadFailed as i32);
}
