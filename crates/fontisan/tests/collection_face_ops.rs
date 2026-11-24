//! Collection faces through the face-0 ops (#192 gap 5 follow-up): the
//! loader hands Ruby's instancer face 0 of a collection, so the engine's
//! instance/glyph_ids must run that face. A `ttcf` wrapping one variable
//! font must instance and look up exactly like the font itself.

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

/// Safety: `r` is a live success result from `call`.
fn payload(r: &FontisanResult) -> Vec<u8> {
    unsafe { std::slice::from_raw_parts(r.payload, r.payload_len) }.to_vec()
}

fn instance(font: &[u8], tag: [u8; 4], value: f64) -> (i32, *mut FontisanResult) {
    let mut input = 1u32.to_be_bytes().to_vec(); // axis count
    input.extend_from_slice(&tag);
    input.extend_from_slice(&((value * 65536.0) as i32).to_be_bytes()); // 16.16
    input.extend_from_slice(font);
    call(&request(OP_INSTANCE, &input))
}

fn glyph_ids(font: &[u8], name: &str, codepoints: &[u32]) -> (i32, *mut FontisanResult) {
    let mut input = (name.len() as u32).to_be_bytes().to_vec();
    input.extend_from_slice(name.as_bytes());
    input.extend_from_slice(&(codepoints.len() as u32).to_be_bytes());
    for cp in codepoints {
        input.extend_from_slice(&cp.to_be_bytes());
    }
    input.extend_from_slice(font);
    call(&request(OP_GLYPH_IDS, &input))
}

/// A one-member `ttcf`: header, offset entry, then the font with its table
/// offsets rebased to absolute positions (a collection member's table
/// offsets point into the whole file, not the member).
fn one_member_ttc(font: &[u8]) -> Vec<u8> {
    let n = u16::from_be_bytes([font[4], font[5]]) as usize;
    let mut face = font.to_vec();
    for i in 0..n {
        let at = 12 + 16 * i + 8; // the table's offset field
        let off = u32::from_be_bytes(face[at..at + 4].try_into().unwrap()) + 16;
        face[at..at + 4].copy_from_slice(&off.to_be_bytes());
    }
    let mut ttc = b"ttcf".to_vec();
    ttc.extend_from_slice(&1u16.to_be_bytes()); // version
    ttc.extend_from_slice(&0u16.to_be_bytes());
    ttc.extend_from_slice(&1u32.to_be_bytes()); // member count
    ttc.extend_from_slice(&16u32.to_be_bytes()); // member offset (after the offset entry)
    ttc.extend_from_slice(&face);
    ttc
}

#[test]
fn instances_face_zero_of_a_wrapped_collection() {
    let Some(font) =
        corpus().and_then(|c| std::fs::read(c.join("fonts/NotoSans[wdth,wght].ttf")).ok())
    else {
        return;
    };
    let ttc = one_member_ttc(&font);
    let (status_a, a) = instance(&font, *b"wght", 500.0);
    let (status_b, b) = instance(&ttc, *b"wght", 500.0);
    assert_eq!(status_a, FontisanStatus::Ok as i32, "raw font failed");
    assert_eq!(status_b, FontisanStatus::Ok as i32, "wrapped ttc failed");
    let (bytes_a, bytes_b) = (
        payload(unsafe { a.as_ref().unwrap() }),
        payload(unsafe { b.as_ref().unwrap() }),
    );
    unsafe {
        fontisan_result_free(a);
        fontisan_result_free(b);
    }
    assert_eq!(
        bytes_a, bytes_b,
        "the wrapped face instances to the same bytes"
    );
    assert!(!bytes_a.is_empty());
}

#[test]
fn glyph_ids_run_face_zero_of_a_wrapped_collection() {
    let Some(font) =
        corpus().and_then(|c| std::fs::read(c.join("fonts/NotoSans-Regular.ttf")).ok())
    else {
        return;
    };
    let ttc = one_member_ttc(&font);
    let name = "NotoSans-Regular.ttf";
    let (status_a, a) = glyph_ids(&font, name, &[65, 66]);
    let (status_b, b) = glyph_ids(&ttc, name, &[65, 66]);
    assert_eq!(status_a, FontisanStatus::Ok as i32);
    assert_eq!(status_b, FontisanStatus::Ok as i32);
    let (bytes_a, bytes_b) = (
        payload(unsafe { a.as_ref().unwrap() }),
        payload(unsafe { b.as_ref().unwrap() }),
    );
    unsafe {
        fontisan_result_free(a);
        fontisan_result_free(b);
    }
    assert_eq!(bytes_a, bytes_b, "the wrapped face looks up the same ids");
    assert!(!bytes_a.is_empty());
}
