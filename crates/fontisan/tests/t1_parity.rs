//! Type 1 surfaces (#192 gap 4, read side): the engine reproduces the Ruby
//! facade's measured Type 1 behavior byte for byte — the deterministic
//! NoMethodError-shaped surfaces after a successful parse, the PFB
//! segment-walk errors before it, and the tolerated audit_dir row.
//! Skips without the corpus, like the other parity suites.
mod common;

use common::corpus;
use fontisan::ffi::*;
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

/// Safety: `r` is a live result from `call`.
fn payload(r: &FontisanResult) -> Vec<u8> {
    unsafe { std::slice::from_raw_parts(r.payload, r.payload_len) }.to_vec()
}

fn corpus_font(name: &str) -> Option<Vec<u8>> {
    std::fs::read(corpus()?.join("fonts").join(name)).ok()
}

const HAS_TABLE: &str =
    "Fontisan::InvalidFontError: Malformed font data (NoMethodError): undefined method \
     'has_table?' for an instance of Fontisan::Type1Font";
const TABLE: &str =
    "Fontisan::InvalidFontError: Malformed font data (NoMethodError): undefined method \
     'table' for an instance of Fontisan::Type1Font";
const TABLE_DATA: &str =
    "Fontisan::InvalidFontError: Malformed font data (NoMethodError): undefined method \
     'table_data' for an instance of Fontisan::Type1Font";

#[test]
fn read_audit_and_outline_surfaces_name_has_table() {
    let Some(font) = corpus_font("fonttools-test-t1-pfb/TestT1-Regular.pfb") else {
        return;
    };
    // OP_READ: the ReadFailed row carries the facade's message.
    let (status, out) = call(&request(OP_READ, &font, READ_FLAG_METADATA | READ_FLAG_LAZY));
    assert_eq!(status, FontisanStatus::ReadFailed as i32);
    let r = unsafe { out.as_ref().unwrap() };
    let text = message(r);
    unsafe { fontisan_result_free(out) };
    assert!(text.contains("InvalidTrueTypeFont") || text.contains("has_table?"), "{text}");
    // The read document's error JSON: class InvalidFontError + the surface.
    assert!(text.contains("\"class\":\"Fontisan::InvalidFontError\""), "{text}");
    assert!(text.contains("undefined method 'has_table?'"), "{text}");

    // OP_AUDIT: the same surface through the audit op.
    let mut input = (3u32).to_be_bytes().to_vec();
    input.extend_from_slice(b"t.ttf");
    input.extend_from_slice(&font);
    let (status, out) = call(&request(OP_AUDIT, &input, 0));
    assert_eq!(status, FontisanStatus::ReadFailed as i32);
    let r = unsafe { out.as_ref().unwrap() };
    let text = message(r);
    unsafe { fontisan_result_free(out) };
    assert!(text.contains("undefined method 'has_table?'"), "{text}");

    // outline_hashes: ConvertFailed with the same surface.
    let mut input = 0u32.to_be_bytes().to_vec();
    input.extend_from_slice(&[0u8; 12]);
    input.extend_from_slice(&font);
    let (status, out) = call(&request(OP_OUTLINE_HASHES, &input, 0));
    assert_eq!(status, FontisanStatus::ConvertFailed as i32);
    let r = unsafe { out.as_ref().unwrap() };
    let text = message(r);
    unsafe { fontisan_result_free(out) };
    assert_eq!(text, HAS_TABLE, "{text}");
}

#[test]
fn glyph_lookup_and_instance_surfaces() {
    let Some(font) = corpus_font("fonttools-test-t1-pfb/TestT1-Regular.pfb") else {
        return;
    };
    // glyph_ids: 'table'.
    let name = "t.pfb";
    let mut input = (name.len() as u32).to_be_bytes().to_vec();
    input.extend_from_slice(name.as_bytes());
    input.extend_from_slice(&1u32.to_be_bytes());
    input.extend_from_slice(&65u32.to_be_bytes());
    input.extend_from_slice(&font);
    let (status, out) = call(&request(OP_GLYPH_IDS, &input, 0));
    assert_eq!(status, FontisanStatus::ConvertFailed as i32);
    let r = unsafe { out.as_ref().unwrap() };
    let text = message(r);
    unsafe { fontisan_result_free(out) };
    assert_eq!(text, TABLE);

    // instance: 'table_data'.
    let mut input = 0u32.to_be_bytes().to_vec();
    input.extend_from_slice(&font);
    let (status, out) = call(&request(OP_INSTANCE, &input, 0));
    assert_eq!(status, FontisanStatus::ConvertFailed as i32);
    let r = unsafe { out.as_ref().unwrap() };
    let text = message(r);
    unsafe { fontisan_result_free(out) };
    assert_eq!(text, TABLE_DATA);

    // subset: the gid lookup runs first → 'table'.
    let mut input = 1u32.to_be_bytes().to_vec();
    input.extend_from_slice(&1u32.to_be_bytes());
    input.extend_from_slice(&[0u8; 8]);
    input.extend_from_slice(&36u32.to_be_bytes());
    input.extend_from_slice(&font);
    let (status, out) = call(&request(OP_SUBSET, &input, 0));
    assert_eq!(status, FontisanStatus::ConvertFailed as i32);
    let r = unsafe { out.as_ref().unwrap() };
    let text = message(r);
    unsafe { fontisan_result_free(out) };
    assert_eq!(text, TABLE);
}

#[test]
fn convert_wraps_table_data_in_the_transform_prefix() {
    let Some(font) = corpus_font("fonttools-test-t1-pfb/TestT1-Regular.pfb") else {
        return;
    };
    let mut input = 0u32.to_be_bytes().to_vec(); // ttf
    input.extend_from_slice(&0u32.to_be_bytes());
    input.extend_from_slice(&0i64.to_be_bytes());
    input.extend_from_slice(&font);
    let (status, out) = call(&request(OP_CONVERT, &input, 0));
    assert_eq!(status, FontisanStatus::ConvertFailed as i32);
    let r = unsafe { out.as_ref().unwrap() };
    let text = message(r);
    unsafe { fontisan_result_free(out) };
    assert_eq!(
        text,
        "Fontisan::Error: Conversion failed: Transformation failed: \
         undefined method 'table_data' for an instance of Fontisan::Type1Font"
    );
}

#[test]
fn segment_walk_errors_precede_the_post_parse_surface() {
    // A truncated PFB whose first chunk claims more than exists: the parser's
    // own surface, exactly Ruby's text (the synthetic mirrors the corpus
    // font's first header: 80 01 e6 02 00 00 = ASCII chunk, 742 bytes).
    let (status, out) = call(&request(OP_READ, &[0x80, 0x01, 0xE6, 0x02, 0x00, 0x00], 0));
    assert_eq!(status, FontisanStatus::ReadFailed as i32);
    let r = unsafe { out.as_ref().unwrap() };
    let text = message(r);
    unsafe { fontisan_result_free(out) };
    assert!(
        text.contains("Invalid PFB: ASCII chunk 0 length 742 exceeds remaining data"),
        "{text}"
    );
    // A clean chunk-0-exactly font parses: the post-parse surface instead.
    let Some(font) = corpus_font("fonttools-test-t1-pfb/TestT1-Regular.pfb") else {
        return;
    };
    let cut = font[..748].to_vec();
    let (status, out) = call(&request(OP_READ, &cut, READ_FLAG_METADATA | READ_FLAG_LAZY));
    assert_eq!(status, FontisanStatus::ReadFailed as i32);
    let r = unsafe { out.as_ref().unwrap() };
    let text = message(r);
    unsafe { fontisan_result_free(out) };
    assert!(text.contains("undefined method 'has_table?'"), "{text}");
}

#[test]
fn audit_dir_keeps_the_tolerated_unsupported_row() {
    let Some(font) = corpus_font("fonttools-test-t1-pfb/TestT1-Regular.pfb") else {
        return;
    };
    let dir = std::env::temp_dir().join(format!("fontisan-t1-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("t.pfb"), &font).unwrap();
    let (status, out) = call(
        &request(OP_AUDIT_DIR, dir.to_string_lossy().as_bytes(), 0),
    );
    assert_eq!(status, FontisanStatus::Ok as i32);
    let r = unsafe { out.as_ref().unwrap() };
    let raw = payload(r);
    unsafe { fontisan_result_free(out) };
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(raw.windows(22).any(|w| w == b"\"status\":\"unsupported\""), "{}", String::from_utf8_lossy(&raw));
}
