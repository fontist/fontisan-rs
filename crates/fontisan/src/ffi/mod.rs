//! Versioned C ABI scaffolding (header: `include/fontisan.h`).
//!
//! * Every struct crossing the boundary starts with a [`FontisanHeader`]
//!   (magic, ABI version, struct size). A stale or foreign caller fails loudly
//!   with a non-zero status and an error row; nothing is guessed.
//! * Memory model: see [`result`]. One call allocates, one call frees.
//! * Panics never cross the boundary: [`guard`] turns them into an error row.
//!   This relies on `panic = "unwind"` in the release profile.
#![allow(unsafe_code)]

mod result;

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};

pub use result::{FontisanErrorRow, FontisanResult};

/// "FSAN" as little-endian bytes.
pub const MAGIC: u32 = u32::from_le_bytes(*b"FSAN");
/// Bumped on any layout or semantics change. Callers compiled against another
/// version are rejected.
pub const ABI_VERSION: u32 = 1;

/// Status / error codes. Stable; append only.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontisanStatus {
    Ok = 0,
    NullPointer = 1,
    BadMagic = 2,
    AbiMismatch = 3,
    BadStructSize = 4,
    UnknownOp = 5,
    Panic = 6,
    InvalidArgument = 7,
}

/// Leads every ABI struct.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FontisanHeader {
    pub magic: u32,
    pub abi_version: u32,
    /// `size_of` the enclosing struct as the producer compiled it.
    pub struct_size: u32,
    pub reserved: u32,
}

impl FontisanHeader {
    pub fn current<T>() -> Self {
        Self {
            magic: MAGIC,
            abi_version: ABI_VERSION,
            struct_size: size_of::<T>() as u32,
            reserved: 0,
        }
    }

    /// Validates a caller header against the struct we expect (`T`).
    pub fn check<T>(&self) -> Result<(), FfiError> {
        if self.magic != MAGIC {
            return Err(FfiError::new(
                FontisanStatus::BadMagic,
                format!("bad magic {:#010x}", self.magic),
            ));
        }
        if self.abi_version != ABI_VERSION {
            return Err(FfiError::new(
                FontisanStatus::AbiMismatch,
                format!(
                    "ABI version {} does not match engine ABI version {ABI_VERSION}",
                    self.abi_version
                ),
            ));
        }
        if self.struct_size as usize != size_of::<T>() {
            return Err(FfiError::new(
                FontisanStatus::BadStructSize,
                format!(
                    "struct size {} does not match expected {}",
                    self.struct_size,
                    size_of::<T>()
                ),
            ));
        }
        Ok(())
    }
}

/// Operations. `Echo` exists to exercise the scaffolding; real ops come in 1g.
pub const OP_ECHO: u32 = 0;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FontisanRequest {
    pub header: FontisanHeader,
    pub op: u32,
    pub flags: u32,
    pub input: *const u8,
    pub input_len: usize,
}

/// Error row source inside the engine.
#[derive(Debug)]
pub struct FfiError {
    pub status: FontisanStatus,
    pub message: String,
}

impl FfiError {
    pub fn new(status: FontisanStatus, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

/// Runs `f`, converting a panic into an error row. Every extern entry point
/// goes through this.
pub fn guard<F>(f: F) -> Result<Vec<u8>, FfiError>
where
    F: FnOnce() -> Result<Vec<u8>, FfiError>,
{
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(payload) => Err(FfiError::new(
            FontisanStatus::Panic,
            format!("engine panic: {}", panic_text(&payload)),
        )),
    }
}

fn panic_text(payload: &Box<dyn Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

/// # Safety
/// `req` must be null or point to at least `size_of::<FontisanHeader>()`
/// readable bytes; if the header validates, to a full `FontisanRequest`.
unsafe fn dispatch(req: *const FontisanRequest) -> Result<Vec<u8>, FfiError> {
    if req.is_null() {
        return Err(FfiError::new(
            FontisanStatus::NullPointer,
            "request is null",
        ));
    }
    // Read only the header first: a stale caller's struct may be smaller.
    let header = unsafe { req.cast::<FontisanHeader>().read() };
    header.check::<FontisanRequest>()?;
    let req = unsafe { req.read() };
    match req.op {
        OP_ECHO => {
            if req.input.is_null() && req.input_len != 0 {
                return Err(FfiError::new(
                    FontisanStatus::InvalidArgument,
                    "input is null with non-zero length",
                ));
            }
            if req.input_len == 0 {
                return Ok(Vec::new());
            }
            Ok(unsafe { std::slice::from_raw_parts(req.input, req.input_len) }.to_vec())
        }
        op => Err(FfiError::new(
            FontisanStatus::UnknownOp,
            format!("unknown op {op}"),
        )),
    }
}

/// The ABI version this engine was built with.
#[unsafe(no_mangle)]
pub extern "C" fn fontisan_abi_version() -> u32 {
    ABI_VERSION
}

/// Runs one request. Returns the status; `*out` receives a result block
/// (error row filled on failure) that the caller releases with
/// [`fontisan_result_free`]. `*out` is null only when `out` itself is null.
///
/// # Safety
/// `req` per [`dispatch`]; `out` must be null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fontisan_call(
    req: *const FontisanRequest,
    out: *mut *mut FontisanResult,
) -> i32 {
    if out.is_null() {
        return FontisanStatus::NullPointer as i32;
    }
    let outcome = guard(|| unsafe { dispatch(req) });
    let (status, message, payload) = match outcome {
        Ok(payload) => (FontisanStatus::Ok, String::new(), payload),
        Err(e) => (e.status, e.message, Vec::new()),
    };
    unsafe { *out = result::into_raw(status, &message, payload) };
    status as i32
}

/// Frees a result block and everything it points to. Null is a no-op.
///
/// # Safety
/// `result` must come from [`fontisan_call`] and be freed at most once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fontisan_result_free(result: *mut FontisanResult) {
    if !result.is_null() {
        unsafe { result::free(result) };
    }
}
