//! Ruby-behaviour helpers the audit checks share (kept out of `checks/mod.rs`,
//! which every check card edits).
use super::types::CheckError;
use crate::sfnt::Error;

/// A read failure as the Ruby exception a check would have raised.
pub fn check_error(e: Error) -> CheckError {
    CheckError::new(e.class.ruby_name(), e.message)
}

/// `font.table(tag)` followed by a method call: a table that is listed but not
/// readable is `nil`, so the first method call raises `NoMethodError`.
pub fn need<T>(table: Result<Option<T>, Error>, method: &str) -> Result<T, CheckError> {
    table
        .map_err(check_error)?
        .ok_or_else(|| nil_method(method))
}

/// Ruby 3.4's `NoMethodError` for a call on nil.
pub fn nil_method(method: &str) -> CheckError {
    CheckError::new(
        "NoMethodError",
        format!("undefined method '{method}' for nil"),
    )
}

/// `Integer#to_s(16)`: lowercase, with a leading `-` for negatives.
pub fn hex(v: i64) -> String {
    if v < 0 {
        format!("-{:x}", -v)
    } else {
        format!("{v:x}")
    }
}

/// Ruby `Float#to_s`: shortest round-trip digits, fixed notation for decimal
/// exponents in (-4, 15], otherwise `d.ddde+XX`.
pub fn float_to_s(f: f64) -> String {
    if f == 0.0 {
        return if f.is_sign_negative() { "-0.0" } else { "0.0" }.into();
    }
    let sci = format!("{:e}", f.abs());
    let (mant, exp) = sci.split_once('e').expect("exponent form");
    let exp: i32 = exp.parse().expect("exponent");
    let digits = mant.replace('.', "");
    let decpt = exp + 1;
    let sign = if f < 0.0 { "-" } else { "" };
    let body = if 0 < decpt && decpt <= 15 {
        let n = decpt as usize;
        if digits.len() <= n {
            format!("{digits}{}.0", "0".repeat(n - digits.len()))
        } else {
            format!("{}.{}", &digits[..n], &digits[n..])
        }
    } else if -4 < decpt && decpt <= 0 {
        format!("0.{}{digits}", "0".repeat((-decpt) as usize))
    } else {
        let rest = if digits.len() > 1 { &digits[1..] } else { "0" };
        format!(
            "{}.{rest}e{}{:02}",
            &digits[..1],
            if exp < 0 { '-' } else { '+' },
            exp.abs()
        )
    };
    format!("{sign}{body}")
}

/// A Fixed 16.16 as `Float` (`fixed_to_float`): exact for every raw value.
pub fn fixed(raw: i32) -> f64 {
    f64::from(raw) / 65536.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float_to_s_matches_ruby() {
        // measured with ruby 3.4: 1.0.to_s, 2.3125.to_s, -0.5.to_s, 1.52587890625e-05.to_s,
        // 0.0001.to_s, 100000.0.to_s, 1.0e+16.to_s, 1234567.5.to_s
        for (v, want) in [
            (1.0, "1.0"),
            (2.3125, "2.3125"),
            (-0.5, "-0.5"),
            (1.52587890625e-5, "1.52587890625e-05"),
            (0.0001, "0.0001"),
            (100000.0, "100000.0"),
            (1e14, "100000000000000.0"),
            (1e15, "1.0e+15"),
            (1e16, "1.0e+16"),
            (1234567.5, "1234567.5"),
            (0.0, "0.0"),
            (-32768.0, "-32768.0"),
        ] {
            assert_eq!(float_to_s(v), want, "{v}");
        }
    }

    #[test]
    fn signed_hex() {
        assert_eq!(hex(0x5000), "5000");
        assert_eq!(hex(-0x10000), "-10000");
    }
}
