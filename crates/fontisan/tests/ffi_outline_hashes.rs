//! OP_OUTLINE_HASHES (#192 gap 2): the facade's `outline_hashes` over the raw
//! ABI — the canonical `{"kind","hashes"}` JSON on success, the Ruby facade's
//! error surface on malformed input, and face selection on collections.
//! Cross-checks against the read document's outline digest: the digest is the
//! SHA-256 of the concatenated per-glyph hashes, so the two ops must agree.
//! Skips without the corpus, like the other parity suites (common::corpus).
mod common;

use common::corpus;
use fontisan::ffi::*;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::ptr;

fn request(op: u32, input: &[u8], flags: u32) -> FontisanRequest {
    FontisanRequest {
        header: FontisanHeader::current::<FontisanRequest>(),
        op,
        flags,
        input: input.as_ptr(),
        input_len: input.len(),
    }
}

fn call(req: &FontisanRequest) -> (i32, *mut FontisanResult) {
    let mut out = ptr::null_mut();
    let status = unsafe { fontisan_call(req, &mut out) };
    (status, out)
}

fn message(r: &FontisanResult) -> String {
    String::from_utf8(
        unsafe { std::slice::from_raw_parts(r.error.message, r.error.message_len) }.to_vec(),
    )
    .unwrap()
}

/// Safety: `r` is a live success result from `call`.
fn payload(r: &FontisanResult) -> Vec<u8> {
    unsafe { std::slice::from_raw_parts(r.payload, r.payload_len) }.to_vec()
}

fn outline_hashes(font: &[u8], index: u32) -> (i32, *mut FontisanResult) {
    let mut input = index.to_be_bytes().to_vec();
    input.extend_from_slice(&[0u8; 12]);
    input.extend_from_slice(font);
    call(&request(OP_OUTLINE_HASHES, &input, 0))
}

/// The op's JSON as a `Value`, plus its raw text (canonical shape assertions).
fn hashes(font: &[u8], index: u32) -> (String, Vec<String>) {
    let (status, out) = outline_hashes(font, index);
    assert_eq!(status, FontisanStatus::Ok as i32);
    let r = unsafe { out.as_ref().expect("payload on success") };
    let raw = payload(r);
    unsafe { fontisan_result_free(out) };
    let json: Value = serde_json::from_slice(&raw).expect("JSON payload");
    let kind = json["kind"].as_str().unwrap_or("").to_string();
    let hashes = json["hashes"]
        .as_array()
        .expect("hashes array")
        .iter()
        .map(|h| h.as_str().expect("hex string").to_string())
        .collect();
    (kind, hashes)
}

/// The first face's read document (OP_READ), via the packed-rows payload.
fn read_document(font: &[u8]) -> Value {
    let (status, out) = call(&request(OP_READ, font, 0));
    assert_eq!(status, FontisanStatus::Ok as i32);
    let r = unsafe { out.as_ref().expect("payload on success") };
    let raw = payload(r);
    unsafe { fontisan_result_free(out) };
    let (count, _) = (u32::from_be_bytes(raw[..4].try_into().unwrap()), ());
    let len =
        u32::from_be_bytes(raw[4..8].try_into().unwrap()) as usize;
    let row: Value = serde_json::from_slice(&raw[8..8 + len]).expect("row JSON");
    assert!(count >= 1);
    row
}

fn corpus_font(name: &str) -> Option<Vec<u8>> {
    std::fs::read(corpus()?.join("fonts").join(name)).ok()
}

fn digest_of(hashes: &[String]) -> String {
    let mut concatenated: Vec<u8> = Vec::new();
    for h in hashes {
        concatenated.extend((0..32).map(|i| u8::from_str_radix(&h[i * 2..i * 2 + 2], 16).unwrap()));
    }
    format!("{:x}", Sha256::digest(&concatenated))
}

#[test]
fn glyf_hashes_match_the_read_documents_digest() {
    let Some(font) = corpus_font("NotoSans-Regular.ttf") else {
        return; // corpus() already printed the skip message
    };
    let (kind, hashes) = hashes(&font, 0);
    assert_eq!(kind, "glyf");
    assert!(!hashes.is_empty());
    assert!(hashes.iter().all(|h| h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit())));

    // The read document's `outlines.digest` is SHA-256 over the concatenated
    // per-glyph hash bytes: the same lookup this op renders, so the two ops
    // must agree byte-for-byte.
    let doc = read_document(&font);
    let digest = doc["outlines"]["digest"].as_str().expect("digest");
    assert_eq!(digest_of(&hashes), digest, "per-glyph hashes must compose the digest");
    let count = doc["outlines"]["count"].as_u64().expect("count");
    assert_eq!(count, hashes.len() as u64);
}

#[test]
fn cff_hashes_on_a_cff_font() {
    let Some(font) = corpus_font("NotoSansJP-Regular.otf") else {
        return;
    };
    let (kind, hashes) = hashes(&font, 0);
    assert_eq!(kind, "cff");
    assert!(!hashes.is_empty());
    let doc = read_document(&font);
    assert_eq!(digest_of(&hashes), doc["outlines"]["digest"].as_str().expect("digest"));
}

#[test]
fn face_select_on_a_collection() {
    let Some(font) = corpus_font("fonttools-test-ttc/TestTTF.ttc") else {
        return;
    };
    let (kind0, hashes0) = hashes(&font, 0);
    let (_, hashes1) = hashes(&font, 1);
    assert_eq!(kind0, "glyf");
    assert!(hashes0 != hashes1, "faces of the collection carry their own glyphs");
    let (status, out) = outline_hashes(&font, 99);
    assert_eq!(status, FontisanStatus::ConvertFailed as i32);
    let r = unsafe { out.as_ref().expect("error on failure") };
    assert!(message(r).contains("index"), "{}", message(r));
    unsafe { fontisan_result_free(out) };
}

#[test]
fn unreadable_font_surfaces_the_facade_class() {
    let Some(mut font) = corpus_font("NotoSans-Regular.ttf") else {
        return;
    };
    font.truncate(24);
    let (status, out) = outline_hashes(&font, 0);
    assert_eq!(status, FontisanStatus::ConvertFailed as i32);
    let r = unsafe { out.as_ref().expect("error on failure") };
    assert!(
        message(r).starts_with("RuntimeError: Invalid TrueTypeFont file:"),
        "{}",
        message(r)
    );
    unsafe { fontisan_result_free(out) };
}

#[test]
fn malformed_request_is_loud() {
    let (status, out) = call(&request(OP_OUTLINE_HASHES, &[0, 0, 0, 1], 0));
    assert_eq!(status, FontisanStatus::InvalidArgument as i32);
    let r = unsafe { out.as_ref().expect("error on failure") };
    assert!(message(r).contains("shorter than"), "{}", message(r));
    unsafe { fontisan_result_free(out) };
}
