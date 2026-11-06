//! Stable stand-in for the nightly cargo-fuzz job (`fuzz/`, card 1j-parsers).
//!
//! Runs the very same target bodies (`fuzz/bodies.rs`) over the committed seeds
//! (`fuzz/seeds/`) plus deterministic mutations: truncations, byte flips,
//! u16/u32 field smashes and chunk splices. A panic in any parser fails the
//! test and names the target, seed and mutation, with the input saved under
//! `$CARGO_TARGET_TMPDIR` for replay. Lengthen the run with
//! `FONTISAN_FUZZ_ROUNDS=<mutations per seed>` (default 250).
//!
//! `FONTISAN_FUZZ_WRITE_SEEDS=1 cargo test --test fuzz_mutation write_seeds -- --ignored`
//! regenerates the derived seeds (WOFF/WOFF2 wrappers, table-capped corpus fonts).
mod common;

#[path = "../../../fuzz/bodies.rs"]
mod bodies;

use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

fn seeds_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/seeds")
}

fn read_seeds() -> Vec<(String, Vec<u8>)> {
    let mut v: Vec<_> = fs::read_dir(seeds_dir())
        .expect("fuzz/seeds missing")
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_file())
        .map(|p| {
            (
                p.file_name().unwrap().to_string_lossy().into_owned(),
                fs::read(&p).unwrap(),
            )
        })
        .collect();
    v.sort();
    v
}

/// xorshift64*: deterministic, no dependency.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// (label, bytes) mutations of `seed`, always the same for the same inputs.
fn mutations(name: &str, seed: &[u8], rounds: usize) -> Vec<(String, Vec<u8>)> {
    let mut rng = Rng(name.bytes().fold(0x9E37_79B9_7F4A_7C15, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100_0000_01B3)
    }));
    let mut out = Vec::with_capacity(rounds + 80);
    let len = seed.len();
    // Truncations: every byte of the first 128, then ~64 points across the rest.
    for n in (0..len.min(128)).chain((0..64).map(|i| 128 + (len.saturating_sub(128)) * i / 64)) {
        out.push((format!("truncate-{n}"), seed[..n.min(len)].to_vec()));
    }
    if len == 0 {
        return out;
    }
    for i in 0..rounds {
        let mut m = seed.to_vec();
        let edits = 1 + rng.below(4);
        let mut label = String::new();
        for _ in 0..edits {
            // Bias towards headers and directories, where counts and offsets live.
            let pos = if rng.below(2) == 0 {
                rng.below(len.min(512))
            } else {
                rng.below(len)
            };
            match rng.below(5) {
                0 => {
                    m[pos] ^= 1 << rng.below(8);
                    label += &format!(" flip@{pos}");
                }
                1 => {
                    m[pos] = rng.next() as u8;
                    label += &format!(" set@{pos}");
                }
                2 => {
                    let v: [u8; 2] = [[0, 0], [0xFF, 0xFF], [0x7F, 0xFF], [0x80, 0]][rng.below(4)];
                    for (k, b) in v.iter().enumerate() {
                        if let Some(d) = m.get_mut(pos + k) {
                            *d = *b;
                        }
                    }
                    label += &format!(" u16@{pos}");
                }
                3 => {
                    let v: [u8; 4] = [
                        [0, 0, 0, 0],
                        [0xFF; 4],
                        [0x7F, 0xFF, 0xFF, 0xFF],
                        [0x80, 0, 0, 0],
                    ][rng.below(4)];
                    for (k, b) in v.iter().enumerate() {
                        if let Some(d) = m.get_mut(pos + k) {
                            *d = *b;
                        }
                    }
                    label += &format!(" u32@{pos}");
                }
                _ => {
                    let to = (pos + rng.below(64)).min(len);
                    let from = rng.below(len);
                    let chunk: Vec<u8> = seed[from..(from + (to - pos)).min(len)].to_vec();
                    m.splice(pos..pos + chunk.len().min(len - pos), chunk);
                    label += &format!(" splice@{pos}");
                }
            }
        }
        out.push((format!("mut-{i}{label}"), m));
    }
    out
}

/// Hang guard: the run that has been going longer than `HANG_SECS` is saved for
/// replay and the whole test process exits non-zero (a hang is a finding too).
const HANG_SECS: u64 = 20;
type Current = std::sync::Mutex<Option<(std::time::Instant, String, Vec<u8>)>>;
static CURRENT: Current = std::sync::Mutex::new(None);

fn start_watchdog() {
    std::thread::spawn(|| {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
            if let Some((t, what, input)) = CURRENT.lock().unwrap().as_ref() {
                if t.elapsed().as_secs() >= HANG_SECS {
                    let file = Path::new(env!("CARGO_TARGET_TMPDIR")).join("hang-input");
                    let _ = fs::write(&file, input);
                    eprintln!(
                        "HANG (> {HANG_SECS}s): {what}; input saved to {}",
                        file.display()
                    );
                    std::process::exit(101);
                }
            }
        }
    });
}

/// Big seeds cost proportionally more per run; scale their share down (min 100).
fn rounds_for(rounds: usize, len: usize) -> usize {
    (rounds * 8192 / len.max(1)).clamp(100.min(rounds), rounds)
}

fn run(target: &str, body: fn(&[u8]), seed: &str, label: &str, input: &[u8]) -> Option<String> {
    *CURRENT.lock().unwrap() = Some((
        std::time::Instant::now(),
        format!("{target} on {seed} [{label}]"),
        input.to_vec(),
    ));
    let r = catch_unwind(AssertUnwindSafe(|| body(input)));
    *CURRENT.lock().unwrap() = None;
    r.err().map(|p| {
        let msg = p
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| p.downcast_ref::<String>().cloned())
            .unwrap_or_default();
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
        let file = dir.join(format!("crash-{target}-{}", seed.replace('/', "_")));
        let _ = fs::write(&file, input);
        format!(
            "{target} on {seed} [{label}]: {msg} (input: {})",
            file.display()
        )
    })
}

#[test]
fn seeds_are_committed_and_cover_every_target() {
    let seeds = read_seeds();
    assert!(seeds.len() >= 15, "only {} seeds", seeds.len());
    for ext in [".ttf", ".ttc", ".woff", ".woff2"] {
        assert!(seeds.iter().any(|(n, _)| n.ends_with(ext)), "no {ext} seed");
    }
    // Every cargo-fuzz target has a body here, and vice versa.
    let mut on_disk: Vec<String> =
        fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/fuzz_targets"))
            .unwrap()
            .map(|e| {
                e.unwrap()
                    .path()
                    .file_stem()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
    on_disk.sort();
    let mut listed: Vec<String> = bodies::TARGETS.iter().map(|(n, _)| n.to_string()).collect();
    listed.sort();
    assert_eq!(on_disk, listed);
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            rust_sources(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Words following `needle` at the start of each non-comment line's match.
fn idents_after(src: &str, needle: &str) -> Vec<String> {
    src.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .filter_map(|l| l.split_once(needle).map(|(_, r)| r))
        .map(|r| {
            r.chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect()
        })
        .collect()
}

/// 1j-abi: every `extern "C"` function and every `OP_*` constant is fuzzed by a
/// listed target, every listed target exists in all three places (TARGETS, the
/// cargo-fuzz bin list, the CI matrix), and the tables name nothing stale.
#[test]
fn every_extern_symbol_and_op_is_fuzzed() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_sources(&root.join("../../crates"), &mut files);
    files.sort();
    files.dedup();
    let mut symbols = Vec::new();
    let mut ops = Vec::new();
    for f in &files {
        let src = fs::read_to_string(f).unwrap();
        symbols.extend(idents_after(&src, "extern \"C\" fn "));
        if f.ends_with("src/ffi/mod.rs") {
            ops.extend(
                idents_after(&src, "pub const ")
                    .into_iter()
                    .filter(|n| n.starts_with("OP_")),
            );
        }
    }
    symbols.sort();
    symbols.dedup();
    ops.sort();
    ops.dedup();
    let mut listed: Vec<&str> = bodies::EXTERN_SYMBOLS.iter().map(|(n, _)| *n).collect();
    listed.sort();
    assert_eq!(
        symbols, listed,
        "extern \"C\" symbols in the crate vs EXTERN_SYMBOLS"
    );
    assert!(symbols.len() >= 3, "symbol scan found {symbols:?}");
    let mut listed_ops: Vec<&str> = bodies::OPS.iter().map(|(n, _, _)| *n).collect();
    listed_ops.sort();
    assert_eq!(ops, listed_ops, "OP_* constants in ffi/mod.rs vs OPS");
    let mut values: Vec<u32> = bodies::OPS.iter().map(|(_, v, _)| *v).collect();
    values.sort();
    values.dedup();
    assert_eq!(values.len(), bodies::OPS.len(), "two ops share a value");

    let cargo = fs::read_to_string(root.join("../../fuzz/Cargo.toml")).unwrap();
    let ci = fs::read_to_string(root.join("../../.github/workflows/ci.yml")).unwrap();
    let matrix = ci
        .lines()
        .find(|l| l.trim_start().starts_with("target: ["))
        .expect("fuzz matrix line");
    let matrix: Vec<&str> = matrix
        .split(['[', ']', ','])
        .skip(1)
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect();
    let mut want: Vec<&str> = bodies::TARGETS.iter().map(|(n, _)| *n).collect();
    want.sort();
    let mut got = matrix.clone();
    got.sort();
    assert_eq!(got, want, "CI fuzz matrix vs TARGETS");
    for name in &want {
        assert!(
            cargo.contains(&format!("name = \"{name}\"")),
            "{name} missing from fuzz/Cargo.toml"
        );
    }
    let in_targets = |t: &str| bodies::TARGETS.iter().any(|(n, _)| *n == t);
    for (sym, targets) in bodies::EXTERN_SYMBOLS {
        assert!(!targets.is_empty(), "{sym} has no fuzz target");
        for t in *targets {
            assert!(in_targets(t), "{sym} names unknown target {t}");
        }
    }
    for (op, _, t) in bodies::OPS {
        assert!(in_targets(t), "{op} names unknown target {t}");
    }
}

#[test]
fn no_parser_panics_on_seeds_or_mutations() {
    let rounds: usize = std::env::var("FONTISAN_FUZZ_ROUNDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(250);
    // Panics are reported below with context; keep the default hook's backtraces quiet.
    std::panic::set_hook(Box::new(|_| {}));
    start_watchdog();
    let mut failures = Vec::new();
    let mut runs = 0usize;
    for (name, seed) in read_seeds() {
        let started = std::time::Instant::now();
        let inputs = std::iter::once(("seed".to_string(), seed.clone())).chain(mutations(
            &name,
            &seed,
            rounds_for(rounds, seed.len()),
        ));
        for (label, input) in inputs {
            for (target, body) in bodies::TARGETS {
                runs += 1;
                if let Some(f) = run(target, *body, &name, &label, &input) {
                    failures.push(f);
                }
            }
        }
        eprintln!(
            "fuzz_mutation: {name} ({} bytes) {:.1?}",
            seed.len(),
            started.elapsed()
        );
    }
    let _ = std::panic::take_hook();
    let oks: Vec<usize> = bodies::OK_BY_OP
        .iter()
        .map(|c| c.load(std::sync::atomic::Ordering::Relaxed))
        .collect();
    eprintln!("fuzz_mutation: successful calls per op {oks:?}");
    for (name, value, target) in bodies::OPS {
        if bodies::NO_SUCCESS_SEED.contains(value) {
            continue;
        }
        assert!(
            oks[*value as usize] > 0,
            "{target}: {name} never succeeded, its well-formed request is broken"
        );
    }
    eprintln!(
        "fuzz_mutation: {runs} target runs, {} failures",
        failures.len()
    );
    assert!(
        failures.is_empty(),
        "{} panics, first 10:\n{}",
        failures.len(),
        failures
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// Rebuild an sfnt with every table capped at `cap` bytes (keeps structure
/// prefixes of big corpus fonts small enough to commit and to mutate).
fn cap_tables(sfnt: &[u8], cap: usize) -> Option<Vec<u8>> {
    let n = u16::from_be_bytes(sfnt.get(4..6)?.try_into().ok()?) as usize;
    let mut tables = Vec::new();
    for i in 0..n {
        let r = sfnt.get(12 + 16 * i..12 + 16 * i + 16)?;
        let off = u32::from_be_bytes(r[8..12].try_into().ok()?) as usize;
        let len = u32::from_be_bytes(r[12..16].try_into().ok()?) as usize;
        let data = sfnt.get(off..off.checked_add(len)?)?;
        tables.push((r[0..4].to_vec(), data[..data.len().min(cap)].to_vec()));
    }
    let mut out = sfnt[..12].to_vec();
    let mut off = 12 + 16 * n;
    let mut body = Vec::new();
    for (tag, d) in &tables {
        out.extend_from_slice(tag);
        out.extend_from_slice(&0u32.to_be_bytes());
        out.extend_from_slice(&((off + body.len()) as u32).to_be_bytes());
        out.extend_from_slice(&(d.len() as u32).to_be_bytes());
        body.extend_from_slice(d);
        body.resize(body.len() + (4 - d.len() % 4) % 4, 0);
    }
    let _ = &mut off;
    out.extend(body);
    Some(out)
}

#[test]
#[ignore = "maintenance: regenerates fuzz/seeds"]
fn write_seeds() {
    use fontisan::convert::{Target, convert};
    let dir = seeds_dir();
    fs::create_dir_all(&dir).unwrap();
    let corpus = common::corpus().expect("corpus needed to write seeds");
    let put = |name: &str, bytes: &[u8]| fs::write(dir.join(name), bytes).unwrap();
    for f in fs::read_dir(corpus.join("derived")).unwrap() {
        let p = f.unwrap().path();
        let bytes = fs::read(&p).unwrap();
        if bytes.is_empty() {
            continue;
        }
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        put(&name, &bytes);
        if name.ends_with(".ttf") && !name.starts_with("malformed") {
            for (t, ext) in [(Target::Woff, "woff"), (Target::Woff2, "woff2")] {
                if let Ok(w) = convert(&bytes, t, 0) {
                    put(&format!("{}.{ext}", name.trim_end_matches(".ttf")), &w);
                }
            }
        }
    }
    // Structure prefixes of the big corpus fonts: glyf, CFF, CFF2, fvar/gvar/HVAR/MVAR/STAT, GSUB/GPOS.
    for name in [
        "NotoSans[wdth,wght].ttf",
        "SourceSans3VF-Upright.otf",
        "SourceSans3-Regular.otf",
        "MonaSansVF[wdth,wght,opsz,ital].ttf",
        "TestDFONT.dfont",
    ] {
        let Ok(bytes) = fs::read(corpus.join("fonts").join(name)) else {
            continue;
        };
        if let Some(c) = cap_tables(&bytes, 6 * 1024) {
            let safe: String = name
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '.')
                .collect();
            put(&format!("capped-{safe}"), &c);
        }
    }
}
