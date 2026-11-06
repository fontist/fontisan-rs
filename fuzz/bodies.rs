//! Fuzz target bodies, shared by the cargo-fuzz targets (`fuzz/fuzz_targets/*`)
//! and the stable mutation test (`crates/fontisan/tests/fuzz_mutation.rs`), so the
//! harness code is exercised on every `cargo test`, not only in the nightly job.
//!
//! Contract of every body: any `&[u8]` returns normally (errors are fine, a panic
//! or hang is the finding).
#![allow(dead_code)]

use fontisan::ffi::{self, FontisanHeader, FontisanRequest, FontisanResult, FontisanStatus};
use fontisan::sfnt::{Font, LoadingMode, Options, face_count};
use fontisan::{audit, woff};

/// Faces tried per input; a collection header can claim thousands.
const MAX_FACES: usize = 4;

const ALL_OPTIONS: [Options; 4] = [
    Options {
        mode: LoadingMode::Full,
        lazy: false,
    },
    Options {
        mode: LoadingMode::Full,
        lazy: true,
    },
    Options {
        mode: LoadingMode::Metadata,
        lazy: false,
    },
    Options {
        mode: LoadingMode::Metadata,
        lazy: true,
    },
];

/// Run `f` on every face of `data` in every loading mode.
fn each_font(data: &[u8], mut f: impl FnMut(&Font<'_>)) {
    let faces = face_count(data).unwrap_or(1).min(MAX_FACES);
    for index in 0..faces.max(1) {
        for options in ALL_OPTIONS {
            if let Ok(font) = Font::load(data, index, options) {
                f(&font);
            }
        }
    }
}

/// `Font::load` in all modes and faces, plus the directory view.
pub fn font_load(data: &[u8]) {
    each_font(data, |font| {
        let _ = (font.format(), font.directory_fields(), font.table_names());
        for e in font.entries() {
            let _ = (
                e.tag_str(),
                font.has_table(&e.tag_str()),
                font.table_data(&e.tag_str()),
            );
        }
        let _ = font.table_summaries();
        let _ = font.woff2_header();
    });
}

/// Fixed-layout and name/cmap/post table accessors.
pub fn font_tables(data: &[u8]) {
    each_font(data, |font| {
        let _ = (font.head(), font.hhea(), font.maxp(), font.os2());
        if let Ok(Some(post)) = font.post() {
            let _ = post.glyph_names();
        }
        if let Ok(Some(name)) = font.name() {
            for r in &name.records {
                let _ = name.decode(r);
            }
            for id in 0..=25 {
                let _ = name.english_name(id);
            }
        }
        if let Ok(Some(cmap)) = font.cmap() {
            let _ = (cmap.subtable_formats(), cmap.codepoints());
        }
    });
}

/// glyf/loca, CFF and CFF2 outline parsing.
pub fn font_outlines(data: &[u8]) {
    each_font(data, |font| {
        if let Ok(Some(o)) = font.outlines() {
            let _ = (o.count(), o.hashes(), o.digest());
        }
        let _ = font.outline_summary();
        let _ = font.cff_charstring_count();
    });
}

/// fvar, avar, gvar, HVAR, VVAR, MVAR, STAT.
pub fn font_variations(data: &[u8]) {
    each_font(data, |font| {
        let _ = (font.fvar(), font.avar());
        let _ = (font.gvar_digest(), font.hvar_digest(), font.vvar_digest());
        let _ = (font.mvar_digest(), font.stat_digest());
    });
}

/// GSUB/GPOS script and feature walk; the raw input is also parsed as a table body.
pub fn font_layout(data: &[u8]) {
    let walk = |t: &fontisan::sfnt::LayoutTable| {
        let _ = t.script_features();
    };
    if let Ok(t) = fontisan::sfnt::LayoutTable::from_bytes(data) {
        walk(&t);
    }
    each_font(data, |font| {
        for t in [font.gsub(), font.gpos()].into_iter().flatten().flatten() {
            walk(&t);
        }
    });
}

/// Decode output must itself be loadable without panicking.
fn load_decoded(decoded: &[u8]) {
    font_load(decoded);
}

/// WOFF 1 decode.
pub fn woff1(data: &[u8]) {
    if let Ok(sfnt) = woff::decode_woff(data) {
        load_decoded(&sfnt);
    }
}

/// WOFF2 (single font) decode, both padding flavours.
pub fn woff2(data: &[u8]) {
    if let Ok(sfnt) = woff::decode_woff2(data) {
        load_decoded(&sfnt);
    }
    let _ = woff::decode_woff2_ruby_padding(data);
}

/// WOFF2 collection decode.
pub fn woff2_collection(data: &[u8]) {
    let _ = woff::is_woff2_collection(data);
    if let Ok(ttc) = woff::decode_woff2_collection(data) {
        load_decoded(&ttc);
    }
}

/// Every registered audit check on every loadable face.
pub fn audit_loaded(data: &[u8]) {
    each_font(data, |font| {
        let _ = audit::run_checks(font);
    });
}

// ---------------------------------------------------------------------------
// C ABI targets (card 1j-abi). Every body goes through the real `extern "C"`
// functions. `fontisan_call` catches panics and reports status 6, which
// libFuzzer and the mutation test would never see, so `call` turns that status
// (and every broken result invariant) back into a panic: that is the finding.
// ---------------------------------------------------------------------------

/// Call `fontisan_call`, check the result block, free it, return the status.
/// `req` may be null and `out` may be null; both are part of the contract.
fn call_raw(req: *const FontisanRequest, with_out: bool) -> i32 {
    let mut out: *mut FontisanResult = std::ptr::null_mut();
    let out_ptr = if with_out {
        &mut out as *mut *mut FontisanResult
    } else {
        std::ptr::null_mut()
    };
    // SAFETY: `req` is null or points to a live header/request; `out_ptr` is null or writable.
    let status = unsafe { ffi::fontisan_call(req, out_ptr) };
    if !with_out {
        assert_eq!(status, FontisanStatus::NullPointer as i32, "null out");
        return status;
    }
    assert!(!out.is_null(), "result block missing (status {status})");
    // SAFETY: `out` is a live block returned by `fontisan_call`, freed once below.
    let r = unsafe { &*out };
    let message = if r.error.message_len == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(r.error.message, r.error.message_len) }.to_vec()
    };
    let text = String::from_utf8_lossy(&message).into_owned();
    assert_ne!(status, FontisanStatus::Panic as i32, "engine panic: {text}");
    assert!((0..=10).contains(&status), "status {status} out of range");
    assert_eq!(r.status, status, "result status");
    assert_eq!(r.error.code, status, "error code");
    assert_eq!(
        r.header,
        FontisanHeader::current::<FontisanResult>(),
        "result header"
    );
    assert!(
        std::str::from_utf8(&message).is_ok(),
        "error message not UTF-8"
    );
    if status == 0 {
        assert!(message.is_empty(), "error text on success: {text}");
        if r.payload_len > 0 {
            // Touch the whole payload: an out-of-bounds length shows up here (and under ASan).
            let p = unsafe { std::slice::from_raw_parts(r.payload, r.payload_len) };
            std::hint::black_box(p.iter().fold(0u8, |a, b| a ^ b));
        }
    } else {
        assert_eq!(r.payload_len, 0, "payload on failure");
    }
    unsafe { ffi::fontisan_result_free(out) };
    status
}

/// A well-formed request (current header) for `op`.
fn call_op(op: u32, flags: u32, input: &[u8]) {
    let req = FontisanRequest {
        header: FontisanHeader::current::<FontisanRequest>(),
        op,
        flags,
        input: input.as_ptr(),
        input_len: input.len(),
    };
    let status = call_raw(&req, true);
    if status == 0 {
        OK_BY_OP[(op as usize).min(OK_BY_OP.len() - 1)]
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Successful calls per op (index = op value; the last slot takes anything larger).
/// The mutation test asserts every op succeeded at least once, so a target whose
/// well-formed prefix has rotted cannot pass by only ever hitting the error path.
///
/// OP_INSTANCE is driven by the small variable-font seed `vf-mini-*.ttf`.
pub static OK_BY_OP: [std::sync::atomic::AtomicUsize; 12] =
    [const { std::sync::atomic::AtomicUsize::new(0) }; 12];

fn be32(v: u32) -> [u8; 4] {
    v.to_be_bytes()
}

/// Splits `data` into (raw?, selectors, font). The control bytes sit at the END so a
/// font seed stays a valid font: the last byte is the mode, the `selectors` bytes
/// before it choose the request parameters, and the font is the whole body, selectors included (so a plain font seed stays one and
/// the tail bytes are just more font to mutate).
/// Mode bits 6 and 7 both set = raw: the whole input (minus the mode byte) is the
/// request input verbatim, so the parameter prefix itself is fuzzed.
fn split(data: &[u8], selectors: usize) -> (Option<&[u8]>, [u8; 16], &[u8]) {
    let mode = data.last().copied().unwrap_or(0);
    let body = data.split_last().map_or(&[][..], |(_, b)| b);
    let n = selectors.min(body.len());
    let tail = &body[body.len() - n..];
    let font = body;
    let mut s = [0u8; 16];
    s[..n].copy_from_slice(tail);
    ((mode & 0xC0 == 0xC0).then_some(body), s, font)
}

fn sel(s: [u8; 16], i: usize) -> u8 {
    s.get(i).copied().unwrap_or(0)
}

pub fn ffi_echo(data: &[u8]) {
    let (_, s, rest) = split(data, 4);
    let flags = u32::from_be_bytes([sel(s, 0), sel(s, 1), sel(s, 2), sel(s, 3)]);
    call_op(ffi::OP_ECHO, flags, rest);
}

pub fn ffi_convert(data: &[u8]) {
    let (raw, s, rest) = split(data, 3);
    if let Some(raw) = raw {
        return call_op(ffi::OP_CONVERT, 0, raw);
    }
    let mut input = Vec::new();
    input.extend(be32(u32::from(sel(s, 0) % 8))); // targets 0..=5 plus invalid 6, 7
    input.extend(be32(0));
    input.extend(i64::from(sel(s, 1)).to_be_bytes());
    input.extend_from_slice(rest);
    call_op(ffi::OP_CONVERT, 0, &input);
}

pub fn ffi_extract(data: &[u8]) {
    let (raw, s, rest) = split(data, 2);
    if let Some(raw) = raw {
        return call_op(ffi::OP_EXTRACT, 0, raw);
    }
    let index = if sel(s, 1) & 0x80 != 0 {
        u32::MAX - u32::from(sel(s, 0)) // far out of range
    } else {
        u32::from(sel(s, 0) % 8)
    };
    let mut input = be32(index).to_vec();
    input.extend([0u8; 12]);
    input.extend_from_slice(rest);
    call_op(ffi::OP_EXTRACT, 0, &input);
}

pub fn ffi_woff2_encode(data: &[u8]) {
    let (raw, s, rest) = split(data, 3);
    if let Some(raw) = raw {
        // Raw: flags straight from the data (any Brotli quality byte), prefix and font verbatim.
        let flags = u32::from(sel(s, 0)) | (u32::from(sel(s, 1)) << 8);
        return call_op(ffi::OP_WOFF2_ENCODE, flags, raw);
    }
    // Structured: quality 1..=6 keeps one input fast.
    let flags = u32::from(sel(s, 0) & 1) | ((u32::from(sel(s, 1) % 6) + 1) << 8);
    let mut input = i64::from(sel(s, 2)).to_le_bytes().to_vec();
    input.extend_from_slice(rest);
    call_op(ffi::OP_WOFF2_ENCODE, flags, &input);
}

pub fn ffi_subset(data: &[u8]) {
    let (raw, s, rest) = split(data, 10);
    if let Some(raw) = raw {
        return call_op(ffi::OP_SUBSET, 0, raw);
    }
    let n = 1 + usize::from(sel(s, 1) % 8);
    let mut input = Vec::new();
    input.extend(be32(u32::from(sel(s, 0) % 4))); // profiles 0..=2 plus invalid 3
    input.extend(be32(n as u32));
    input.extend([0u8; 8]);
    for i in 0..n {
        input.extend(be32(u32::from(sel(s, 2 + i) % 48))); // small ids, some past numGlyphs
    }
    input.extend_from_slice(rest);
    call_op(ffi::OP_SUBSET, 0, &input);
}

pub fn ffi_instance(data: &[u8]) {
    const TAGS: [&[u8; 4]; 5] = [b"wght", b"wdth", b"opsz", b"ital", b"slnt"];
    let (raw, s, rest) = split(data, 1 + 3 * 3);
    if let Some(raw) = raw {
        return call_op(ffi::OP_INSTANCE, 0, raw);
    }
    let axes = usize::from(sel(s, 0) % 4);
    let mut input = be32(axes as u32).to_vec();
    for a in 0..axes {
        input.extend(TAGS[usize::from(sel(s, 1 + 3 * a)) % TAGS.len()]);
        let user = i32::from(i16::from_be_bytes([sel(s, 2 + 3 * a), sel(s, 3 + 3 * a)]));
        input.extend((user * 16).to_be_bytes()); // 16.16, about +-32768
    }
    input.extend_from_slice(rest);
    call_op(ffi::OP_INSTANCE, 0, &input);
}

pub fn ffi_read(data: &[u8]) {
    let (raw, s, rest) = split(data, 1);
    if let Some(raw) = raw {
        return call_op(ffi::OP_READ, u32::from(sel(s, 0)), raw);
    }
    call_op(ffi::OP_READ, u32::from(sel(s, 0) & 3), rest);
}

pub fn ffi_audit(data: &[u8]) {
    const NAME: &[u8] = b"fuzz-input.ttf";
    let (raw, s, rest) = split(data, 1);
    if let Some(raw) = raw {
        return call_op(ffi::OP_AUDIT, u32::from(sel(s, 0)), raw);
    }
    let mut input = be32(NAME.len() as u32).to_vec();
    input.extend(NAME);
    input.extend_from_slice(rest);
    call_op(ffi::OP_AUDIT, u32::from(sel(s, 0) & 1), &input);
}

/// Extensions the scanner recognises, plus one it must ignore.
const SCAN_EXTS: [&str; 9] = [
    "ttf", "otf", "ttc", "otc", "woff", "woff2", "dfont", "pfb", "txt",
];

/// A scratch directory holding the fuzz bytes as font files (top level and nested).
/// SCAN / AUDIT_DIR only ever see paths inside it, never arbitrary host paths.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(font: &[u8], ext_a: u8, ext_b: u8) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "fontisan-fuzz-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let ext = |b: u8| SCAN_EXTS[usize::from(b) % SCAN_EXTS.len()];
        std::fs::create_dir_all(dir.join("sub")).expect("scratch dir");
        std::fs::write(dir.join(format!("a.{}", ext(ext_a))), font).expect("scratch file");
        std::fs::write(dir.join("sub").join(format!("b.{}", ext(ext_b))), font)
            .expect("scratch file");
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn path_op(op: u32, data: &[u8], flags_from_data: bool) {
    let (_, s, rest) = split(data, 3);
    let scratch = Scratch::new(rest, sel(s, 0), sel(s, 1));
    // Bit 0: the directory, the file inside it, or a path that does not exist.
    let path = match sel(s, 2) % 3 {
        0 => scratch.0.clone(),
        1 => scratch.0.join("sub"),
        _ => scratch.0.join("missing"),
    };
    let flags = if flags_from_data {
        u32::from((sel(s, 2) >> 2) & 1)
    } else {
        0
    };
    call_op(op, flags, path.to_string_lossy().as_bytes());
}

pub fn ffi_scan(data: &[u8]) {
    path_op(ffi::OP_SCAN, data, false);
}

pub fn ffi_audit_dir(data: &[u8]) {
    path_op(ffi::OP_AUDIT_DIR, data, true);
}

/// The header, pointers and lengths. Derived from `data`: magic, version, struct
/// size, reserved, op, flags (each valid or wild), a null or real input pointer, a
/// null request, a null `out`, a stale request struct that is only 16 bytes long,
/// and `fontisan_abi_version` / `fontisan_result_free(NULL)`.
pub fn ffi_header(data: &[u8]) {
    let mut c = data.iter().copied();
    let mut u32_of =
        |c: &mut dyn Iterator<Item = u8>| u32::from_le_bytes([0; 4].map(|_| c.next().unwrap_or(0)));
    let modes = c.next().unwrap_or(0);
    let wild = |bit: u8,
                valid: u32,
                c: &mut dyn Iterator<Item = u8>,
                f: &mut dyn FnMut(&mut dyn Iterator<Item = u8>) -> u32| {
        if (modes >> bit) & 1 == 0 { valid } else { f(c) }
    };
    let magic = wild(0, ffi::MAGIC, &mut c, &mut u32_of);
    let abi_version = wild(1, ffi::ABI_VERSION, &mut c, &mut u32_of);
    let struct_size = wild(2, size_of::<FontisanRequest>() as u32, &mut c, &mut u32_of);
    let reserved = u32_of(&mut c);
    let op = u32_of(&mut c) % 16; // 0..=11 valid ops, 12..=15 unknown
    let flags = u32_of(&mut c);
    let rest: Vec<u8> = c.collect();
    let header = FontisanHeader {
        magic,
        abi_version,
        struct_size,
        reserved,
    };

    assert_eq!(ffi::fontisan_abi_version(), ffi::ABI_VERSION);
    // SAFETY: null is a documented no-op.
    unsafe { ffi::fontisan_result_free(std::ptr::null_mut()) };
    match (modes >> 3) & 7 {
        0 => {
            call_raw(std::ptr::null(), true);
        }
        1 => {
            call_raw(std::ptr::null(), false);
        }
        2 => {
            let req = FontisanRequest {
                header,
                op,
                flags,
                input: rest.as_ptr(),
                input_len: rest.len(),
            };
            call_raw(&req, false);
        }
        3 if header.check::<FontisanRequest>().is_err() => {
            // A stale caller's struct is smaller than ours: only the 16-byte header exists.
            let bytes: Box<[FontisanHeader; 1]> = Box::new([header]);
            call_raw(bytes.as_ptr().cast(), true);
        }
        4 => {
            // Null input pointer with any length (including non-zero).
            let req = FontisanRequest {
                header,
                op,
                flags,
                input: std::ptr::null(),
                input_len: usize::from(rest.first().copied().unwrap_or(0)),
            };
            call_raw(&req, true);
        }
        _ => {
            let req = FontisanRequest {
                header,
                op,
                flags,
                input: rest.as_ptr(),
                input_len: rest.len(),
            };
            // Ops that take a path would read host paths; keep the wild header/op mix
            // away from them (they have their own scratch-dir targets).
            if !matches!(op, ffi::OP_SCAN | ffi::OP_AUDIT_DIR) {
                call_raw(&req, true);
            }
        }
    }
}

/// Every `extern "C"` function in the crate and the targets that call it. A test
/// compares this with the sources, so a new symbol cannot be added unfuzzed.
pub const EXTERN_SYMBOLS: &[(&str, &[&str])] = &[
    ("fontisan_abi_version", &["ffi_header"]),
    ("fontisan_call", &["ffi_header"]),
    ("fontisan_result_free", &["ffi_header"]),
];

/// Every `OP_*` constant, its value (from the crate, so a renumbering is followed)
/// and the target that sends it. Targets call the op through this table's constant.
pub const OPS: &[(&str, u32, &str)] = &[
    ("OP_ECHO", ffi::OP_ECHO, "ffi_echo"),
    ("OP_CONVERT", ffi::OP_CONVERT, "ffi_convert"),
    ("OP_EXTRACT", ffi::OP_EXTRACT, "ffi_extract"),
    ("OP_WOFF2_ENCODE", ffi::OP_WOFF2_ENCODE, "ffi_woff2_encode"),
    ("OP_SUBSET", ffi::OP_SUBSET, "ffi_subset"),
    ("OP_INSTANCE", ffi::OP_INSTANCE, "ffi_instance"),
    ("OP_READ", ffi::OP_READ, "ffi_read"),
    ("OP_AUDIT", ffi::OP_AUDIT, "ffi_audit"),
    ("OP_SCAN", ffi::OP_SCAN, "ffi_scan"),
    ("OP_AUDIT_DIR", ffi::OP_AUDIT_DIR, "ffi_audit_dir"),
];

/// (name, body) for every target; the mutation test iterates this, and its length
/// is compared against `fuzz/fuzz_targets/` so a target cannot be left untested.
pub type Body = fn(&[u8]);
pub const TARGETS: &[(&str, Body)] = &[
    ("font_load", font_load),
    ("font_tables", font_tables),
    ("font_outlines", font_outlines),
    ("font_variations", font_variations),
    ("font_layout", font_layout),
    ("woff1", woff1),
    ("woff2", woff2),
    ("woff2_collection", woff2_collection),
    ("audit_loaded", audit_loaded),
    ("ffi_echo", ffi_echo),
    ("ffi_convert", ffi_convert),
    ("ffi_extract", ffi_extract),
    ("ffi_woff2_encode", ffi_woff2_encode),
    ("ffi_subset", ffi_subset),
    ("ffi_instance", ffi_instance),
    ("ffi_read", ffi_read),
    ("ffi_audit", ffi_audit),
    ("ffi_scan", ffi_scan),
    ("ffi_audit_dir", ffi_audit_dir),
    ("ffi_header", ffi_header),
];
