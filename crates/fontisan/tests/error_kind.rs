//! Error kinds (round 2, item 3 and F5): every engine error carries a stable
//! [`ErrorKind`] through the Rust API and `fontisan_result.error_kind`, so a caller maps by
//! kind, not message. An unknown axis is `InvalidAxis`, distinct from the coordinate and
//! generic kinds. Corpus-backed cases skip (and say so) without the corpus.
mod common;

use common::{corpus, font_path};
use fontisan::ffi::*;
use fontisan::{ErrorKind, convert, instance, subset};

fn ffi(op: u32, flags: u32, input: &[u8]) -> (i32, i32, String) {
    let req = FontisanRequest {
        header: FontisanHeader::current::<FontisanRequest>(),
        op,
        flags,
        input: input.as_ptr(),
        input_len: input.len(),
    };
    let mut out = std::ptr::null_mut();
    let status = unsafe { fontisan_call(&req, &mut out) };
    assert!(!out.is_null(), "engine must always set the result block");
    let r = unsafe { &*out };
    let msg = unsafe { std::slice::from_raw_parts(r.error.message, r.error.message_len) };
    let got = (
        status,
        r.error_kind,
        String::from_utf8_lossy(msg).into_owned(),
    );
    unsafe { fontisan_result_free(out) };
    got
}

fn instance_input(axes: &[(&[u8; 4], i32)], font: &[u8]) -> Vec<u8> {
    let mut v = (axes.len() as u32).to_be_bytes().to_vec();
    for (tag, raw) in axes {
        v.extend_from_slice(*tag);
        v.extend_from_slice(&raw.to_be_bytes());
    }
    v.extend_from_slice(font);
    v
}

#[test]
fn success_has_kind_none_and_result_struct_announces_the_field() {
    let req = FontisanRequest {
        header: FontisanHeader::current::<FontisanRequest>(),
        op: OP_ECHO,
        flags: 0,
        input: b"x".as_ptr(),
        input_len: 1,
    };
    let mut out = std::ptr::null_mut();
    assert_eq!(unsafe { fontisan_call(&req, &mut out) }, 0);
    assert!(!out.is_null(), "engine must always set the result block");
    let r = unsafe { &*out };
    assert_eq!(r.error_kind, ErrorKind::None as i32);
    assert!(r.header.struct_size as usize >= 72 || cfg!(not(target_pointer_width = "64")));
    unsafe { fontisan_result_free(out) };
}

#[test]
fn unknown_axis_is_invalid_axis_through_api_and_abi() {
    let Some(c) = corpus() else { return };
    let font = std::fs::read(font_path(&c, "SourceSans3VF-Upright.ttf")).unwrap();
    let e = instance::instance(&font, &[(*b"zzzz", 1.0)]).unwrap_err();
    assert_eq!(e.class, "Fontisan::InvalidAxisError");
    assert_eq!(e.kind(), ErrorKind::InvalidAxis);

    let (status, kind, msg) = ffi(
        OP_INSTANCE,
        0,
        &instance_input(&[(b"zzzz", 1 << 16)], &font),
    );
    assert_eq!(status, FontisanStatus::ConvertFailed as i32);
    assert_eq!(kind, ErrorKind::InvalidAxis as i32, "{msg}");
    assert!(msg.starts_with("Fontisan::InvalidAxisError: Invalid variation axis 'zzzz'"));
}

#[test]
fn out_of_range_coordinate_is_invalid_coordinates() {
    let Some(c) = corpus() else { return };
    let font = std::fs::read(font_path(&c, "SourceSans3VF-Upright.ttf")).unwrap();
    let (_, kind, msg) = ffi(
        OP_INSTANCE,
        0,
        &instance_input(&[(b"wght", 5000 << 16)], &font),
    );
    assert_eq!(kind, ErrorKind::InvalidCoordinates as i32, "{msg}");
    // Distinct kinds: neither is the generic one.
    assert_ne!(ErrorKind::InvalidAxis, ErrorKind::InvalidCoordinates);
}

#[test]
fn convert_subset_and_read_errors_have_kinds() {
    let e = convert::convert(b"not a font at all......", convert::Target::Woff, 0).unwrap_err();
    assert_eq!(e.kind(), ErrorKind::from_class(e.class));
    let e = subset::subset(&[0; 12], &[1], "pdf").unwrap_err();
    assert_eq!(e.kind(), ErrorKind::from_class(e.class));

    // Through the ABI: unsupported / invalid argument / read failure.
    let (s, k, _) = ffi(99, 0, b"");
    assert_eq!(
        (s, k),
        (FontisanStatus::UnknownOp as i32, ErrorKind::Other as i32)
    );
    let (s, k, _) = ffi(OP_SUBSET, 0, b"short");
    assert_eq!(
        (s, k),
        (
            FontisanStatus::InvalidArgument as i32,
            ErrorKind::InvalidArgument as i32
        )
    );
    let (s, k, m) = ffi(OP_READ, 0, b"\x00\x01\x00\x00garbage");
    assert_eq!(s, FontisanStatus::ReadFailed as i32, "{m}");
    assert_ne!(k, ErrorKind::None as i32);
    assert_ne!(k, ErrorKind::Other as i32, "{m}");
}
