//! Bounds-checked big-endian reads. Ruby would raise `NoMethodError` on a nil
//! slice; here a truncated table is a typed error.

use crate::convert::{ConvertError, ERROR_CLASS};

pub(crate) type R<T> = Result<T, ConvertError>;

pub(crate) fn bad() -> ConvertError {
    ConvertError::new(
        ERROR_CLASS,
        "Static instancing: truncated or malformed table",
    )
}

pub(crate) fn sl(d: &[u8], pos: usize, len: usize) -> R<&[u8]> {
    let end = pos.checked_add(len).ok_or_else(bad)?;
    d.get(pos..end).ok_or_else(bad)
}

pub(crate) fn u8_(d: &[u8], pos: usize) -> R<u8> {
    d.get(pos).copied().ok_or_else(bad)
}

pub(crate) fn u16_(d: &[u8], pos: usize) -> R<u16> {
    let b = sl(d, pos, 2)?;
    Ok(u16::from_be_bytes([b[0], b[1]]))
}

pub(crate) fn i16_(d: &[u8], pos: usize) -> R<i16> {
    Ok(u16_(d, pos)? as i16)
}

pub(crate) fn u32_(d: &[u8], pos: usize) -> R<u32> {
    let b = sl(d, pos, 4)?;
    Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

pub(crate) fn i32_(d: &[u8], pos: usize) -> R<i32> {
    Ok(u32_(d, pos)? as i32)
}

/// Ruby `Float#to_s` for the values that reach messages.
pub(crate) fn rb_float(v: f64) -> String {
    if v.is_finite() && v == v.trunc() && v.abs() < 1e16 {
        format!("{v:.1}")
    } else {
        format!("{v}")
    }
}
