//! dfont (fontisan#192 gap 5): the resource-fork reader against the Ruby
//! tier's surfaces, captured on the corpus dfont — read/scan/audit rows,
//! outline hashes, the glyph_ids first-face semantics and its no-match
//! error, index errors, the truncated-map Malformed surface, and the
//! conversion targets' ArgumentError.
mod common;

use common::corpus;
use fontisan::ffi::*;
use serde_json::Value;
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

/// Safety: `r` is a live result from `call`.
fn rows(r: &FontisanResult) -> Vec<Value> {
    let raw = unsafe { std::slice::from_raw_parts(r.payload, r.payload_len) }.to_vec();
    let count = u32::from_be_bytes(raw[..4].try_into().unwrap()) as usize;
    let mut out = Vec::with_capacity(count);
    let mut at = 4;
    for _ in 0..count {
        let len = u32::from_be_bytes(raw[at..at + 4].try_into().unwrap()) as usize;
        out.push(serde_json::from_slice(&raw[at + 4..at + 4 + len]).expect("row JSON"));
        at += 4 + len;
    }
    out
}

fn message(r: &FontisanResult) -> String {
    String::from_utf8(
        unsafe { std::slice::from_raw_parts(r.error.message, r.error.message_len) }.to_vec(),
    )
    .unwrap()
}

fn corpus_font(name: &str) -> Option<Vec<u8>> {
    std::fs::read(corpus()?.join("fonts").join(name)).ok()
}

#[test]
fn read_and_outline_hashes_match_the_captured_goldens() {
    let Some(font) = corpus_font("TestDFONT.dfont") else {
        return;
    };
    // Reference.read rows (captured from the Ruby tier, metadata+lazy —
    // Ruby's dfont collection path defaults the face read to metadata mode).
    let (status, out) = call(&request(
        OP_READ,
        &font,
        READ_FLAG_METADATA | READ_FLAG_LAZY,
    ));
    assert_eq!(status, FontisanStatus::Ok as i32);
    let r = unsafe { out.as_ref().expect("payload") };
    let docs = rows(r);
    unsafe { fontisan_result_free(out) };
    assert_eq!(docs.len(), 1, "one face");
    assert_eq!(docs[0]["format"], "dfont");
    assert_eq!(
        docs[0]["file_sha256"],
        "acacaa44936ec33623ced5210365a584e103afd0a37f72b0b0e9b30ea7f7b97c"
    );
    // The whole read document, when the captured golden is present.
    if let Ok(golden) = std::fs::read("/tmp/dfont-read.json") {
        let golden: Value = serde_json::from_slice(&golden).unwrap();
        assert_eq!(
            docs[0], golden[0],
            "the full read document must match Ruby's"
        );
    }

    // Reference.outline_hashes (captured from the Ruby tier).
    let mut input = 0u32.to_be_bytes().to_vec();
    input.extend_from_slice(&[0u8; 12]);
    input.extend_from_slice(&font);
    let (status, out) = call(&request(OP_OUTLINE_HASHES, &input, 0));
    assert_eq!(status, FontisanStatus::Ok as i32);
    let r = unsafe { out.as_ref().expect("payload") };
    let json: Value =
        serde_json::from_slice(unsafe { std::slice::from_raw_parts(r.payload, r.payload_len) })
            .unwrap();
    unsafe { fontisan_result_free(out) };
    let golden: Value =
        serde_json::from_slice(&std::fs::read("/tmp/dfont-outline.json").unwrap_or_default())
            .unwrap_or(Value::Null);
    if !golden.is_null() {
        assert_eq!(json, golden, "outline hashes must equal the Ruby tier's");
    }
    assert!(!json["hashes"].as_array().expect("hashes").is_empty());
}

#[test]
fn glyph_ids_runs_the_first_face_with_the_file_basename() {
    let Some(font) = corpus_font("TestDFONT.dfont") else {
        return;
    };
    let name = "TestDFONT.dfont";
    let mut input = (name.len() as u32).to_be_bytes().to_vec();
    input.extend_from_slice(name.as_bytes());
    input.extend_from_slice(&2u32.to_be_bytes());
    input.extend_from_slice(&65u32.to_be_bytes()); // 'A'
    input.extend_from_slice(&66u32.to_be_bytes()); // 'B'
    input.extend_from_slice(&font);
    let (status, out) = call(&request(OP_GLYPH_IDS, &input, 0));
    // Ruby: the face's cmap has neither A nor B → the no-match error names
    // the dfont file.
    assert_eq!(status, FontisanStatus::ConvertFailed as i32);
    let r = unsafe { out.as_ref().expect("error on failure") };
    assert_eq!(
        message(r),
        "Fontisan::Error: None of the requested characters exist in TestDFONT.dfont"
    );
    unsafe { fontisan_result_free(out) };
}

#[test]
fn index_and_structure_errors_match_the_parser() {
    let Some(font) = corpus_font("TestDFONT.dfont") else {
        return;
    };
    // Out-of-range index through OP_READ's per-face loop: the read op fails
    // whole, like Ruby's read raising for the face.
    // (OP_READ itself only reads existing faces; check via outlines' index.)
    let mut input = 99u32.to_be_bytes().to_vec();
    input.extend_from_slice(&[0u8; 12]);
    input.extend_from_slice(&font);
    let (status, out) = call(&request(OP_OUTLINE_HASHES, &input, 0));
    assert_eq!(status, FontisanStatus::ConvertFailed as i32);
    let r = unsafe { out.as_ref().expect("error on failure") };
    assert!(
        message(r).contains("Font index 99 out of range (dfont has 1 fonts)"),
        "{}",
        message(r)
    );
    unsafe { fontisan_result_free(out) };

    // A structurally detected dfont whose map walk runs off the file: Ruby's
    // nil.unpack1 NoMethodError through the malformed-font wrapper.
    let mut truncated = font.clone();
    truncated.truncate(24);
    let (status, out) = call(&request(OP_READ, &truncated, 0));
    assert_eq!(status, FontisanStatus::ReadFailed as i32);
    let r = unsafe { out.as_ref().expect("error on failure") };
    assert!(
        message(r).contains("undefined method 'unpack1' for nil"),
        "{}",
        message(r)
    );
    unsafe { fontisan_result_free(out) };

    // The dfont magic without the structural check is an unknown format
    // (Ruby rejects a 12-byte "dfont").
    let mut tiny = font[..12.min(font.len())].to_vec();
    let (status, out) = call(&request(OP_READ, &tiny, 0));
    tiny.clear();
    assert_eq!(status, FontisanStatus::ReadFailed as i32);
    let r = unsafe { out.as_ref().expect("error on failure") };
    assert!(message(r).contains("Unknown font format"), "{}", message(r));
    unsafe { fontisan_result_free(out) };
}

#[test]
fn convert_reproduces_the_collection_converter_argument_error() {
    let Some(font) = corpus_font("TestDFONT.dfont") else {
        return;
    };
    // Ruby: a dfont source goes through CollectionConverter, which rejects
    // single-font targets.
    let mut input = 0u32.to_be_bytes().to_vec(); // ttf
    input.extend_from_slice(&0u32.to_be_bytes());
    input.extend_from_slice(&0i64.to_be_bytes());
    input.extend_from_slice(&font);
    let (status, out) = call(&request(OP_CONVERT, &input, 0));
    assert_eq!(status, FontisanStatus::ConvertFailed as i32);
    let r = unsafe { out.as_ref().expect("error on failure") };
    assert_eq!(
        message(r),
        "ArgumentError: Target format ttf is not a collection format. Use ttc, otc, or dfont for collection conversion."
    );
    unsafe { fontisan_result_free(out) };
}
