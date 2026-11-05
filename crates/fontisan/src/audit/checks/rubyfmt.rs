//! Helpers shared by the audit checks that reproduce the Ruby reference's
//! observable behaviour: exception mapping and `Float#to_s`.

use crate::audit::types::CheckError;
use crate::sfnt::Error;

/// A table-parse failure as the exception Ruby would have raised from `font.table(tag)`.
pub(crate) fn sfnt_err(e: Error) -> CheckError {
    CheckError::new(e.class.ruby_name(), e.message)
}

/// `font.table(tag)` for a table the directory lists: a parse failure raises, a nil
/// result (table data past EOF) makes the next method call raise `NoMethodError`.
pub(crate) fn required<T>(parsed: Result<Option<T>, Error>, method: &str) -> Result<T, CheckError> {
    match parsed {
        Ok(Some(t)) => Ok(t),
        Ok(None) => Err(nil_method(method)),
        Err(e) => Err(sfnt_err(e)),
    }
}

/// Ruby 3.4's `NoMethodError` for a call on nil.
pub(crate) fn nil_method(method: &str) -> CheckError {
    CheckError::new(
        "NoMethodError",
        format!("undefined method '{method}' for nil"),
    )
}

/// Ruby's `Fixed 16.16 -> Float` (`fixed_to_float`): exact in f64.
pub(crate) fn fixed(raw: i32) -> f64 {
    f64::from(raw) / 65536.0
}

/// Ruby's `Float#to_s`: shortest round-trip digits, fixed notation for decimal
/// exponents in (-4, 15], otherwise `d.ddde+XX`.
pub(crate) fn ruby_float(x: f64) -> String {
    if x == 0.0 {
        return if x.is_sign_negative() { "-0.0" } else { "0.0" }.into();
    }
    let sign = if x < 0.0 { "-" } else { "" };
    let sci = format!("{:e}", x.abs());
    let (mant, exp) = sci.split_once('e').expect("exponent form");
    let digits: String = mant.chars().filter(|c| *c != '.').collect();
    let decpt = exp.parse::<i32>().expect("exponent") + 1;
    let body = if 0 < decpt && decpt <= 15 {
        let d = decpt as usize;
        if digits.len() <= d {
            format!("{digits}{}.0", "0".repeat(d - digits.len()))
        } else {
            format!("{}.{}", &digits[..d], &digits[d..])
        }
    } else if decpt <= 0 && decpt > -4 {
        format!("0.{}{digits}", "0".repeat((-decpt) as usize))
    } else {
        let rest = if digits.len() > 1 { &digits[1..] } else { "0" };
        let e = decpt - 1;
        format!(
            "{}.{rest}e{}{:02}",
            &digits[..1],
            if e < 0 { '-' } else { '+' },
            e.abs()
        )
    };
    format!("{sign}{body}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float_to_s_matches_ruby() {
        // Expected strings measured with Ruby 3.4 `Float#to_s`.
        for (x, s) in [
            (0.0, "0.0"),
            (100.0, "100.0"),
            (400.5, "400.5"),
            (2.3125, "2.3125"),
            (-0.5, "-0.5"),
            (0.0001, "0.0001"),
            (0.00001, "1.0e-05"),
            (1.0 / 65536.0, "1.52587890625e-05"),
            (32767.99998474121, "32767.99998474121"),
            (1e14, "100000000000000.0"),
            (1e15, "1.0e+15"),
            (1e16, "1.0e+16"),
        ] {
            assert_eq!(ruby_float(x), s, "{x}");
        }
    }
}
