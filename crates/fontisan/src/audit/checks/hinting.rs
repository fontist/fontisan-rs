//! `hinting` (Ruby `HintingCheck`): TrueType hint programs against `maxp`,
//! `gasp`, and the CFF Private DICT hinting hints.
//!
//! CFF quirks reproduced from the reference:
//!  * Ruby's `font.table("CFF2")` is nil (`Tables::Cff` is only mapped for
//!    `CFF `, and only for OpenType-flavored `OTTO` fonts), so CFF2 and
//!    TrueType-flavored fonts give no issue.
//!  * `Cff.read` parses eagerly and its failure raises out of the check
//!    (`Fontisan::CorruptedTableError: Failed to parse CFF table: ...`); only
//!    the Private DICT lookup is rescued (no issue).
//!  * has_blues needs an Array with at least one operand (one operand is a
//!    scalar); has_stems needs a scalar non-zero number (an Array raises on
//!    `zero?`, rescued to false).

use super::rubyfmt::required;
use crate::audit::types::{CheckError, Issue, Severity};
use crate::sfnt::Font;

pub const CODE: &str = "hinting";
const OTTO: u32 = 0x4F54_544F;

pub fn run(font: &Font<'_>) -> Result<Vec<Issue>, CheckError> {
    if font.has_table("glyf") {
        truetype(font)
    } else if font.has_table("CFF ") || font.has_table("CFF2") {
        cff(font)
    } else {
        Ok(Vec::new())
    }
}

fn has_data(font: &Font<'_>, tag: &str) -> bool {
    font.table_data(tag).is_some_and(|d| !d.is_empty())
}

fn no_program(table: &str) -> Issue {
    let what = if table == "cvt" {
        "control values"
    } else {
        "pre-program"
    };
    Issue::new(
        CODE,
        Severity::Info,
        format!(
            "Font has 'fpgm' but no '{table}' table \u{2014} {what} are recommended for consistent hinting"
        ),
    )
    .at(format!("tables.{table}"))
}

fn truetype(font: &Font<'_>) -> Result<Vec<Issue>, CheckError> {
    let mut issues = Vec::new();
    let has_fpgm = font.has_table("fpgm");
    if has_fpgm && has_data(font, "glyf") {
        let maxp = required(font.maxp(), "version_1_0?")?;
        if let Some(v1) = maxp.v1 {
            let (zones, stack) = (v1[4], v1[9]);
            if has_data(font, "fpgm") && stack == 0 {
                issues.push(
                    Issue::new(
                        CODE,
                        Severity::Warning,
                        "maxp.maxStackElements is 0 but the font has hinting instructions \u{2014} rendering may fail",
                    )
                    .at("maxp.max_stack_elements"),
                );
            }
            if !(1..=2).contains(&zones) {
                issues.push(
                    Issue::new(
                        CODE,
                        Severity::Warning,
                        format!(
                            "maxp.maxZones is {zones} but must be 1 or 2 per the OpenType spec"
                        ),
                    )
                    .at("maxp.max_zones"),
                );
            }
        }
    }
    if !font.has_table("prep") && has_fpgm {
        issues.push(no_program("prep"));
    }
    if !font.has_table("cvt ") && has_fpgm {
        issues.push(no_program("cvt"));
    }
    if !font.has_table("gasp") {
        issues.push(
            Issue::new(
                CODE,
                Severity::Info,
                "TrueType font has no 'gasp' table \u{2014} grid-fitting behavior per ppem is implementation-defined",
            )
            .at("tables.gasp"),
        );
    }
    Ok(issues)
}

fn cff(font: &Font<'_>) -> Result<Vec<Issue>, CheckError> {
    if font.has_table("CFF2") || font.sfnt_version != OTTO {
        return Ok(Vec::new());
    }
    let Some(data) = font.table_data("CFF ") else {
        return Ok(Vec::new());
    };
    let top_dicts = cff::parse(data).map_err(|m| {
        CheckError::new(
            "Fontisan::CorruptedTableError",
            format!("Failed to parse CFF table: {m}"),
        )
    })?;
    let Some(private) = top_dicts
        .first()
        .and_then(|top| cff::private_dict(data, top))
    else {
        return Ok(Vec::new());
    };
    let has_blues = [6u16, 7]
        .iter()
        .any(|op| matches!(private.get(*op), Some(cff::Value::Array(a)) if !a.is_empty()));
    let has_stems = [10u16, 11].iter().any(|op| match private.get(*op) {
        Some(cff::Value::Scalar(n)) => !n.is_zero(),
        _ => false,
    });
    if has_blues || has_stems {
        return Ok(Vec::new());
    }
    Ok(vec![
        Issue::new(
            CODE,
            Severity::Info,
            "CFF Private DICT has no BlueValues or StdHW/StdVW \u{2014} alignment zones and stem widths are the primary hinting mechanism for CFF outlines",
        )
        .at("cff.private_dict"),
    ])
}

/// The slice of `Fontisan::Tables::Cff` the check touches: structure parse
/// (Header, Name/Top DICT/String/Global Subr INDEXes, Top DICTs) and the first
/// font's Private DICT. Error strings are Ruby's.
mod cff {
    #[derive(Clone, Copy, Debug)]
    pub enum Num {
        Int(i64),
        Real(f64),
    }

    impl Num {
        pub fn is_zero(self) -> bool {
            match self {
                Num::Int(i) => i == 0,
                Num::Real(r) => r == 0.0,
            }
        }
        fn as_f64(self) -> f64 {
            match self {
                Num::Int(i) => i as f64,
                Num::Real(r) => r,
            }
        }
    }

    #[derive(Clone, Debug)]
    pub enum Value {
        Scalar(Num),
        Array(Vec<Num>),
    }

    /// Operator code: the byte, or `0x0C00 | second` for escaped operators.
    pub struct Dict(Vec<(u16, Value)>);

    impl Dict {
        /// Hash semantics: the last assignment of an operator wins.
        pub fn get(&self, op: u16) -> Option<&Value> {
            self.0.iter().rev().find(|(o, _)| *o == op).map(|(_, v)| v)
        }
    }

    struct Io<'a> {
        data: &'a [u8],
        pos: usize,
    }

    impl<'a> Io<'a> {
        /// `StringIO#read(n)`; `None` when fewer than n bytes remain.
        fn take(&mut self, n: usize) -> Option<&'a [u8]> {
            if n == 0 {
                return Some(&[]);
            }
            let end = self.pos.checked_add(n)?;
            if self.pos >= self.data.len() || end > self.data.len() {
                self.pos = self.data.len().max(self.pos);
                return None;
            }
            let s = &self.data[self.pos..end];
            self.pos = end;
            Some(s)
        }
    }

    const EOF_INDEX: &str = "Unexpected end of INDEX data";

    /// `Cff::Index.new(io, start_offset:)`: returns the item slices.
    fn index<'a>(io: &mut Io<'a>, start: usize) -> Result<Vec<&'a [u8]>, String> {
        io.pos = start;
        let count = {
            let b = io.take(2).ok_or(EOF_INDEX)?;
            usize::from(u16::from_be_bytes([b[0], b[1]]))
        };
        if count == 0 {
            return Ok(Vec::new());
        }
        let off_size = usize::from(io.take(1).ok_or(EOF_INDEX)?[0]);
        if !(1..=4).contains(&off_size) {
            return Err(format!("Invalid INDEX offSize: {off_size} (must be 1-4)"));
        }
        let mut offsets = Vec::with_capacity(count + 1);
        for _ in 0..=count {
            let b = io.take(off_size).ok_or(EOF_INDEX)?;
            offsets.push(b.iter().fold(0usize, |s, &x| (s << 8) | usize::from(x)));
        }
        if offsets[0] != 1 {
            return Err(format!(
                "Invalid INDEX: first offset must be 1, got {}",
                offsets[0]
            ));
        }
        if offsets.windows(2).any(|w| w[1] < w[0]) {
            return Err("Invalid INDEX: offsets are not in ascending order".into());
        }
        let data = io.take(offsets[count] - 1).ok_or(EOF_INDEX)?;
        Ok(offsets
            .windows(2)
            .map(|w| &data[w[0] - 1..w[1] - 1])
            .collect())
    }

    /// `Cff::Dict#parse!` (operand stack cleared after every operator).
    fn dict(data: &[u8]) -> Result<Dict, String> {
        const EOF: &str = "Unexpected end of DICT";
        let mut out = Vec::new();
        let mut stack: Vec<Num> = Vec::new();
        let mut i = 0;
        let byte = |i: &mut usize| -> Result<u8, String> {
            let b = *data.get(*i).ok_or(EOF)?;
            *i += 1;
            Ok(b)
        };
        while i < data.len() {
            let b = data[i];
            i += 1;
            if b <= 21 || b == 12 {
                let op = if b == 12 {
                    0x0C00 | u16::from(byte(&mut i)?)
                } else {
                    u16::from(b)
                };
                let value = if stack.len() == 1 {
                    Value::Scalar(stack[0])
                } else {
                    Value::Array(stack.clone())
                };
                out.push((op, value));
                stack.clear();
                continue;
            }
            let n = match b {
                28 => {
                    let hi = byte(&mut i)?;
                    let lo = byte(&mut i)?;
                    Num::Int(i64::from(i16::from_be_bytes([hi, lo])))
                }
                29 => {
                    let mut w = [0u8; 4];
                    for x in &mut w {
                        *x = byte(&mut i)?;
                    }
                    Num::Int(i64::from(i32::from_be_bytes(w)))
                }
                30 => Num::Real(real(data, &mut i)?),
                32..=246 => Num::Int(i64::from(b) - 139),
                247..=250 => Num::Int((i64::from(b) - 247) * 256 + i64::from(byte(&mut i)?) + 108),
                251..=254 => Num::Int(-(i64::from(b) - 251) * 256 - i64::from(byte(&mut i)?) - 108),
                _ => return Err(format!("Invalid DICT operand byte: {b}")),
            };
            stack.push(n);
        }
        Ok(Dict(out))
    }

    /// Nibble-coded real, then Ruby's lenient `String#to_f`.
    fn real(data: &[u8], i: &mut usize) -> Result<f64, String> {
        let mut s = String::new();
        loop {
            let b = *data.get(*i).ok_or("Unexpected end of DICT")?;
            *i += 1;
            let (hi, lo) = (b >> 4, b & 0x0F);
            for nib in [hi, lo] {
                if nib == 0xF {
                    return Ok(to_f(&s));
                }
                match nib {
                    0..=9 => s.push(char::from(b'0' + nib)),
                    0xa => s.push('.'),
                    0xb => s.push('e'),
                    0xc => s.push_str("e-"),
                    0xe => s.push('-'),
                    _ => {}
                }
            }
        }
    }

    /// `String#to_f`: the longest valid numeric prefix, else 0.0.
    fn to_f(s: &str) -> f64 {
        let b = s.as_bytes();
        let mut j = 0;
        if j < b.len() && b[j] == b'-' {
            j += 1;
        }
        let int_start = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j == int_start {
            return 0.0;
        }
        if j + 1 < b.len() && b[j] == b'.' && b[j + 1].is_ascii_digit() {
            j += 1;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
        }
        if j < b.len() && b[j] == b'e' {
            let mut k = j + 1;
            if k < b.len() && b[k] == b'-' {
                k += 1;
            }
            let ds = k;
            while k < b.len() && b[k].is_ascii_digit() {
                k += 1;
            }
            if k > ds {
                j = k;
            }
        }
        s[..j].parse().unwrap_or(0.0)
    }

    /// `Cff#parse!`: the Top DICTs, or the message Ruby wraps.
    pub fn parse(data: &[u8]) -> Result<Vec<Dict>, String> {
        let mut io = Io { data, pos: 0 };
        let h = io.take(4).ok_or("End of file reached")?;
        let (major, minor, hdr_size, off_size) = (h[0], h[1], h[2], h[3]);
        if !([1, 2].contains(&major) && minor == 0 && hdr_size >= 4 && (1..=4).contains(&off_size))
        {
            return Err(format!(
                "Invalid CFF header: version={major}.{minor}, hdr_size={hdr_size}, off_size={off_size}"
            ));
        }
        let mut pos = if hdr_size > 4 {
            usize::from(hdr_size)
        } else {
            4
        };
        // INDEX.new seeks to its start; after parsing, StringIO sits at the end of its data.
        fn next<'a>(io: &mut Io<'a>, pos: &mut usize) -> Result<Vec<&'a [u8]>, String> {
            let items = index(io, *pos)?;
            *pos = io.pos;
            Ok(items)
        }
        let names = next(&mut io, &mut pos)?;
        if names.is_empty() {
            return Err("CFF table must contain at least one font".into());
        }
        let tops = next(&mut io, &mut pos)?;
        if tops.len() != names.len() {
            return Err(format!(
                "Top DICT count ({}) must match Name count ({})",
                tops.len(),
                names.len()
            ));
        }
        next(&mut io, &mut pos)?; // String INDEX
        next(&mut io, &mut pos)?; // Global Subr INDEX
        tops.into_iter().map(dict).collect()
    }

    /// `Cff#private_dict(0)`; any failure (all rescued by the check) is `None`.
    pub fn private_dict(data: &[u8], top: &Dict) -> Option<Dict> {
        let (size, offset) = match top.get(18)? {
            Value::Scalar(_) => return None, // offset.negative? on nil raises
            Value::Array(a) => (*a.first()?, *a.get(1)?),
        };
        if size.as_f64() <= 0.0 || offset.as_f64() < 0.0 {
            return None;
        }
        let (offset, size) = (offset.as_f64() as usize, size.as_f64() as usize);
        if offset > data.len() {
            return None;
        }
        dict(&data[offset..offset.saturating_add(size).min(data.len())]).ok()
    }
}
