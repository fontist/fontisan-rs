//! ABI scaffolding: layout, loud failures on stale headers, panic containment
//! and the memory model. CI additionally runs these suites under
//! AddressSanitizer (Linux nightly). The allocator check lives in ffi_leak.rs.
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

#[test]
fn layout_matches_header_file() {
    // Mirrors include/fontisan.h on 64-bit targets.
    assert_eq!(size_of::<FontisanHeader>(), 16);
    if cfg!(target_pointer_width = "64") {
        assert_eq!(size_of::<FontisanRequest>(), 40);
        assert_eq!(size_of::<FontisanResult>(), 72);
    }
    assert_eq!(MAGIC, 0x4E41_5346);
    assert_eq!(fontisan_abi_version(), ABI_VERSION);
}

#[test]
fn echo_roundtrips_and_frees_in_one_call() {
    let (status, out) = call(&request(OP_ECHO, b"hello"));
    assert_eq!(status, 0);
    let r = unsafe { &*out };
    assert_eq!(
        unsafe { std::slice::from_raw_parts(r.payload, r.payload_len) },
        b"hello"
    );
    unsafe { fontisan_result_free(out) };
}

#[test]
fn stale_version_fails_loudly() {
    let mut req = request(OP_ECHO, b"x");
    req.header.abi_version = ABI_VERSION - 1;
    let (status, out) = call(&req);
    assert_eq!(status, FontisanStatus::AbiMismatch as i32);
    let r = unsafe { &*out };
    assert_eq!(r.status, status);
    assert!(message(r).contains("does not match"), "{}", message(r));
    unsafe { fontisan_result_free(out) };
}

#[test]
fn bad_magic_bad_size_null_and_unknown_op_are_rejected() {
    let mut req = request(OP_ECHO, b"");
    req.header.magic = 0;
    let (s, out) = call(&req);
    assert_eq!(s, FontisanStatus::BadMagic as i32);
    unsafe { fontisan_result_free(out) };

    let mut req = request(OP_ECHO, b"");
    req.header.struct_size -= 8;
    let (s, out) = call(&req);
    assert_eq!(s, FontisanStatus::BadStructSize as i32);
    unsafe { fontisan_result_free(out) };

    let (s, out) = call(&request(99, b""));
    assert_eq!(s, FontisanStatus::UnknownOp as i32);
    unsafe { fontisan_result_free(out) };

    let mut out = ptr::null_mut();
    assert_eq!(
        unsafe { fontisan_call(ptr::null(), &mut out) },
        FontisanStatus::NullPointer as i32
    );
    unsafe { fontisan_result_free(out) };
    assert_eq!(
        unsafe { fontisan_call(ptr::null(), ptr::null_mut()) },
        FontisanStatus::NullPointer as i32
    );
    unsafe { fontisan_result_free(ptr::null_mut()) };
}

#[test]
fn panic_becomes_error_row() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let r = guard(|| -> Result<Vec<u8>, FfiError> { panic!("boom") });
    std::panic::set_hook(prev);
    let e = r.unwrap_err();
    assert_eq!(e.status, FontisanStatus::Panic);
    assert!(e.message.contains("boom"));
}
