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
    /// Conversion or extract failed; the message is `<ruby class>: <message>`.
    ConvertFailed = 8,
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
/// Convert. `input` = 16-byte prefix (u32 BE target: 0 ttf, 1 otf, 2 woff,
/// 3 woff2; u32 reserved; i64 BE timestamp) followed by the font bytes.
/// Payload: the converted font.
pub const OP_CONVERT: u32 = 1;
/// Extract a collection member. `input` = 16-byte prefix (u32 BE index; 12
/// reserved bytes) followed by the collection. Payload: u32 BE basename
/// length, the basename, then the member's font bytes.
pub const OP_EXTRACT: u32 = 2;
/// Encode a single-font sfnt as WOFF2 (card 1h-woff2enc). `input` is an 8-byte little-endian
/// i64 Unix timestamp followed by the sfnt bytes; `flags` bit 0 set = use the timestamp for
/// `head.modified`, clear = use the clock. The payload is the WOFF2 file.
pub const OP_WOFF2_ENCODE: u32 = 3;
/// `flags` bits 8..=15 of [`OP_WOFF2_ENCODE`]: Brotli quality + 1 (0 = the default, 11).
/// `flags` bit for [`OP_WOFF2_ENCODE`]: the timestamp prefix is meaningful.
pub const WOFF2_FLAG_TIMESTAMP: u32 = 1;

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
        OP_CONVERT | OP_EXTRACT => {
            if req.input.is_null() && req.input_len != 0 {
                return Err(FfiError::new(
                    FontisanStatus::InvalidArgument,
                    "input is null with non-zero length",
                ));
            }
            let input: &[u8] = if req.input_len == 0 {
                &[]
            } else {
                unsafe { std::slice::from_raw_parts(req.input, req.input_len) }
            };
            if input.len() < 16 {
                return Err(FfiError::new(
                    FontisanStatus::InvalidArgument,
                    "input is shorter than the 16-byte parameter prefix",
                ));
            }
            let param = u32::from_be_bytes([input[0], input[1], input[2], input[3]]);
            let mut ts = [0u8; 8];
            ts.copy_from_slice(&input[8..16]);
            let font = &input[16..];
            let failed = |e: crate::convert::ConvertError| {
                FfiError::new(FontisanStatus::ConvertFailed, e.to_string())
            };
            if req.op == OP_CONVERT {
                let target = crate::convert::Target::from_code(param).ok_or_else(|| {
                    FfiError::new(
                        FontisanStatus::InvalidArgument,
                        format!("unknown target {param}"),
                    )
                })?;
                crate::convert::convert(font, target, i64::from_be_bytes(ts)).map_err(failed)
            } else {
                let x = crate::convert::extract(font, param as usize).map_err(failed)?;
                let mut out = Vec::with_capacity(4 + x.basename.len() + x.data.len());
                out.extend_from_slice(&(x.basename.len() as u32).to_be_bytes());
                out.extend_from_slice(x.basename.as_bytes());
                out.extend_from_slice(&x.data);
                Ok(out)
            }
        }
        OP_WOFF2_ENCODE => {
            if req.input.is_null() || req.input_len < 8 {
                return Err(FfiError::new(
                    FontisanStatus::InvalidArgument,
                    "woff2 encode input needs an 8-byte timestamp prefix and a font",
                ));
            }
            let input = unsafe { std::slice::from_raw_parts(req.input, req.input_len) };
            let ts = i64::from_le_bytes(input[..8].try_into().expect("8 bytes"));
            let ts = (req.flags & WOFF2_FLAG_TIMESTAMP != 0).then_some(ts);
            let q = match (req.flags >> 8) & 0xFF {
                0 => crate::convert::woff2::DEFAULT_BROTLI_QUALITY,
                n => (n - 1) as u8,
            };
            crate::convert::woff2::encode_woff2_with_quality(&input[8..], ts, q)
                .map_err(|e| FfiError::new(FontisanStatus::InvalidArgument, e.to_string()))
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
