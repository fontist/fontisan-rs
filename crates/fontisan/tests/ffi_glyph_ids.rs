//! OP_GLYPH_IDS (#192 gap 1): the standalone glyph-id lookup over the raw
//! ABI — sorted, unique ids on success, the Ruby facade's error classes on
//! failure, and loud InvalidArgument on malformed input. Skips without the
//! corpus, like the other parity suites (common::corpus).
mod common;

use common::corpus;
use fontisan::ffi::*;
use std::ptr;

fn request(op: u32, input: &[u8]) -> FontisanRequest {
    FontisanRequest {
        header: FontisanHeader::current::<FontisanRequest>(),
        op,
        flags: 0,
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

fn gids(font: &[u8], name: &str, codepoints: &[u32]) -> (i32, *mut FontisanResult) {
    let mut input = (name.len() as u32).to_be_bytes().to_vec();
    input.extend_from_slice(name.as_bytes());
    input.extend_from_slice(&(codepoints.len() as u32).to_be_bytes());
    for cp in codepoints {
        input.extend_from_slice(&cp.to_be_bytes());
    }
    input.extend_from_slice(font);
    call(&request(OP_GLYPH_IDS, &input))
}

fn be_u32s(raw: &[u8]) -> Vec<u32> {
    raw.chunks_exact(4)
        .map(|c| u32::from_be_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

fn test_ttf() -> Option<Vec<u8>> {
    let path = corpus()?.join("fonts/fonttools-test-ttf/TestTTF.ttf");
    std::fs::read(path).ok()
}

#[test]
fn glyph_ids_returns_sorted_unique_ids() {
    let Some(font) = test_ttf() else {
        return; // corpus() already printed the skip message
    };
    let (status, out) = gids(&font, "TestTTF.ttf", &[46, 46, 65]);
    assert_eq!(status, FontisanStatus::Ok as i32);
    let r = unsafe { out.as_ref().expect("payload on success") };
    let gids = be_u32s(&payload(r));
    unsafe { fontisan_result_free(out) };
    let mut sorted = gids.clone();
    sorted.sort_unstable();
    assert_eq!(gids, sorted, "ids must be sorted");
    let unique = gids.len();
    sorted.dedup();
    assert_eq!(gids.len(), unique, "payload carries the ids");
    assert_eq!(sorted.len(), unique, "ids must be unique");
}

#[test]
fn glyph_ids_matches_the_subset_path() {
    let Some(font) = test_ttf() else {
        return; // corpus() already printed the skip message
    };
    let (status, out) = gids(&font, "TestTTF.ttf", &[46, 65]);
    assert_eq!(status, FontisanStatus::Ok as i32);
    let r = unsafe { out.as_ref().expect("payload on success") };
    let gids = be_u32s(&payload(r));
    unsafe { fontisan_result_free(out) };
    // The subset op consumes exactly these ids, so its own pipeline must
    // accept them end to end.
    let mut input = 1u32.to_be_bytes().to_vec(); // web profile
    input.extend_from_slice(&(gids.len() as u32).to_be_bytes());
    input.extend_from_slice(&[0u8; 8]);
    for gid in &gids {
        input.extend_from_slice(&gid.to_be_bytes());
    }
    input.extend_from_slice(&font);
    let (subset_status, subset_out) = call(&request(OP_SUBSET, &input));
    assert_eq!(subset_status, FontisanStatus::Ok as i32);
    unsafe { fontisan_result_free(subset_out) };
}

#[test]
fn glyph_ids_raises_the_facade_error_when_nothing_matches() {
    let Some(font) = test_ttf() else {
        return; // corpus() already printed the skip message
    };
    let (status, out) = gids(&font, "TestTTF.ttf", &[0x10_FFFF]);
    assert_eq!(status, FontisanStatus::ConvertFailed as i32);
    let r = unsafe { out.as_ref().expect("error on failure") };
    assert!(message(r).contains("None of the requested characters exist"));
    unsafe { fontisan_result_free(out) };
}

#[test]
fn glyph_ids_surfaces_the_facade_class_for_unreadable_fonts() {
    // A truncated sfnt: the facade's glyph_ids raises the canonical
    // RuntimeError("Invalid TrueTypeFont file: ...") — the malformed parity
    // surface — not a generic Fontisan::Error wrapper.
    let mut font = test_ttf().expect("corpus present");
    font.truncate(24);
    let (status, out) = gids(&font, "TestTTF.ttf", &[46]);
    assert_eq!(status, FontisanStatus::ConvertFailed as i32);
    let r = unsafe { out.as_ref().expect("error on failure") };
    assert!(message(r).starts_with("RuntimeError: Invalid TrueTypeFont file:"));
    unsafe { fontisan_result_free(out) };
}

#[test]
fn glyph_ids_loudly_rejects_malformed_input() {
    // Truncated before the codepoint header.
    let (status, out) = call(&request(OP_GLYPH_IDS, &[0, 0, 0, 3]));
    assert_eq!(status, FontisanStatus::InvalidArgument as i32);
    let r = unsafe { out.as_ref().expect("error on failure") };
    assert!(message(r).contains("shorter than"));
    unsafe { fontisan_result_free(out) };
    // A codepoint list longer than the input.
    let font = test_ttf().unwrap_or_default();
    let mut input = 1u32.to_be_bytes().to_vec();
    input.extend_from_slice(b"a");
    input.extend_from_slice(&9_000_000u32.to_be_bytes());
    input.extend_from_slice(&font);
    let (status, out) = call(&request(OP_GLYPH_IDS, &input));
    assert_eq!(status, FontisanStatus::InvalidArgument as i32);
    unsafe { fontisan_result_free(out) };
}
