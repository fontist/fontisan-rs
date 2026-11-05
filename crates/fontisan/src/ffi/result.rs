//! Memory model: the engine allocates one result block per call; the caller
//! frees it with ONE call, `fontisan_result_free`. Every pointer inside the
//! block (error message, payload) is owned by the block and dies with it.
//! The caller never frees anything else and never owns engine memory
//! otherwise.
use super::{FontisanHeader, FontisanStatus};

/// Error row: a stable code plus a UTF-8 message (not NUL-terminated).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FontisanErrorRow {
    pub code: i32,
    pub message: *const u8,
    pub message_len: usize,
}

/// What the caller sees. Layout is part of the ABI; change it only with an
/// `ABI_VERSION` bump.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FontisanResult {
    pub header: FontisanHeader,
    pub status: i32,
    pub error: FontisanErrorRow,
    pub payload: *const u8,
    pub payload_len: usize,
}

/// `view` is first so a `*mut FontisanResult` is also the block's address.
#[repr(C)]
pub(super) struct ResultBlock {
    view: FontisanResult,
    message: Box<[u8]>,
    payload: Box<[u8]>,
}

pub(super) fn into_raw(
    status: FontisanStatus,
    message: &str,
    payload: Vec<u8>,
) -> *mut FontisanResult {
    let message: Box<[u8]> = message.as_bytes().into();
    let payload = payload.into_boxed_slice();
    let view = FontisanResult {
        header: FontisanHeader::current::<FontisanResult>(),
        status: status as i32,
        error: FontisanErrorRow {
            code: status as i32,
            message: message.as_ptr(),
            message_len: message.len(),
        },
        payload: payload.as_ptr(),
        payload_len: payload.len(),
    };
    // Box<[u8]> keeps its heap address when the block moves into the Box.
    Box::into_raw(Box::new(ResultBlock {
        view,
        message,
        payload,
    }))
    .cast()
}

/// # Safety
/// `ptr` must come from `into_raw` and not have been freed.
pub(super) unsafe fn free(ptr: *mut FontisanResult) {
    drop(unsafe { Box::from_raw(ptr.cast::<ResultBlock>()) });
}
