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

/// Ruby `Float#to_s`: `Infinity`, `-Infinity`, `NaN`, fixed notation for decimal
/// exponents in -4..16, otherwise `d.ddde+XX` with at least two exponent digits.
pub(crate) fn rb_float(v: f64) -> String {
    if v.is_nan() {
        return "NaN".into();
    }
    if v.is_infinite() {
        return if v < 0.0 { "-Infinity" } else { "Infinity" }.into();
    }
    if v == 0.0 {
        return if v.is_sign_negative() { "-0.0" } else { "0.0" }.into();
    }
    // `{:e}` is the shortest round-trip digits, like Ruby's.
    let sci = format!("{v:e}");
    let (mant, exp) = sci.split_once('e').expect("exponent form");
    let exp: i32 = exp.parse().expect("exponent");
    let decpt = exp + 1;
    if (-3..=16).contains(&decpt) {
        let fixed = format!("{v}");
        if fixed.contains('.') {
            fixed
        } else {
            format!("{fixed}.0")
        }
    } else {
        let mant = if mant.contains('.') {
            mant.to_string()
        } else {
            format!("{mant}.0")
        };
        let sign = if exp < 0 { '-' } else { '+' };
        format!("{mant}e{sign}{:02}", exp.abs())
    }
}

#[cfg(test)]
mod tests {
    use super::rb_float;

    #[test]
    fn rb_float_matches_ruby_to_s() {
        for (v, want) in [
            (f64::INFINITY, "Infinity"),
            (f64::NEG_INFINITY, "-Infinity"),
            (f64::NAN, "NaN"),
            (0.0, "0.0"),
            (-0.0, "-0.0"),
            (700.0, "700.0"),
            (489.59, "489.59"),
            (1e15, "1000000000000000.0"),
            (1e16, "1.0e+16"),
            (1e-4, "0.0001"),
            (1e-5, "1.0e-05"),
            (1.5e300, "1.5e+300"),
            (-2.5e-7, "-2.5e-07"),
            (1e9, "1000000000.0"),
        ] {
            assert_eq!(rb_float(v), want, "{v:?}");
        }
    }
}
