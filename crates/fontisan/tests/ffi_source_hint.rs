//! The `.ttc`/`.otc` source hint on OP_CONVERT / OP_EXTRACT (#192 gap 7):
//! Ruby decides a `ttcf` input's source type from the path extension, so the
//! caller passes it; the hint decides copy vs repack on same-type conversions
//! and the extract basename (`.ttf` vs `.otf`). A hint-less call (0) keeps the
//! legacy `.ttc` behaviour.
mod common;

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

/// A one-member, one-table `ttcf` (no outlines): the repack path rejects it,
/// the copy path and extract still serve it.
fn synthetic_ttc() -> Vec<u8> {
    let mut ttc = b"ttcf".to_vec();
    ttc.extend([0, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 16]);
    ttc.extend([0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0]); // sfnt header, one table
    ttc.extend(b"AAAA");
    ttc.extend([0; 4]);
    ttc.extend(16u32.to_be_bytes());
    ttc.extend(4u32.to_be_bytes());
    ttc.extend([1, 2, 3, 4]);
    ttc
}

fn prefix(param: u32, source: u32, tail: [u8; 8]) -> Vec<u8> {
    let mut p = param.to_be_bytes().to_vec();
    p.extend_from_slice(&source.to_be_bytes());
    p.extend_from_slice(&tail);
    p
}

fn convert(collection: &[u8], target: u32, source: u32) -> (i32, *mut FontisanResult) {
    let mut input = prefix(target, source, [0; 8]);
    input.extend_from_slice(collection);
    call(&request(OP_CONVERT, &input))
}

fn extract(collection: &[u8], index: u32, source: u32) -> (i32, *mut FontisanResult) {
    let mut input = prefix(index, source, [0; 8]);
    input.extend_from_slice(collection);
    call(&request(OP_EXTRACT, &input))
}

#[test]
fn otc_hint_copies_a_same_type_conversion() {
    let ttc = synthetic_ttc();
    let (status, out) = convert(&ttc, 5 /* otc */, 2 /* otc */);
    assert_eq!(status, FontisanStatus::Ok as i32);
    let r = unsafe { out.as_ref().expect("payload on success") };
    assert_eq!(payload(r), ttc, "same source and target type copies the input");
    unsafe { fontisan_result_free(out) };
}

#[test]
fn legacy_hint_keeps_the_ttc_repack() {
    // Source 0 (the legacy default) is TTC: an otc target repacks the members,
    // and this outline-less member aborts Ruby's detect_font_format.
    let ttc = synthetic_ttc();
    let (status, out) = convert(&ttc, 5 /* otc */, 0);
    assert_eq!(status, FontisanStatus::ConvertFailed as i32);
    let r = unsafe { out.as_ref().expect("error on failure") };
    assert!(message(r).contains("Cannot detect font format"), "{}", message(r));
    unsafe { fontisan_result_free(out) };
}

#[test]
fn extract_basename_follows_the_hint() {
    let ttc = synthetic_ttc();
    for (source, ext) in [(0u32, ".ttf"), (1, ".ttf"), (2, ".otf")] {
        let (status, out) = extract(&ttc, 0, source);
        assert_eq!(status, FontisanStatus::Ok as i32);
        let r = unsafe { out.as_ref().expect("payload on success") };
        let raw = payload(r);
        unsafe { fontisan_result_free(out) };
        let len = u32::from_be_bytes(raw[..4].try_into().expect("length")) as usize;
        let basename = String::from_utf8(raw[4..4 + len].to_vec()).unwrap();
        assert!(
            basename.ends_with(ext),
            "source {source}: {basename} must end with {ext}"
        );
    }
}
