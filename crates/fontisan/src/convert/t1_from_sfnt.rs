//! OTF → Type 1 (fontisan#192 gap 4, stage 2): the op-level conversion the
//! Ruby `CffToType1Converter` performs - parse the Type 2 charstring into
//! operations (stems counted as they arrive, hint masks consumed), rescale
//! coordinates to 1000 upm, extract the odd-stack width, and emit the Type 1
//! bytecode (hsbw prefix, walk, endchar) - then assemble the canonical PFB
//! with the source's names, FontInfo, Private entries and scaled FontBBox.

use crate::convert::t1write::{encode_number, Pfb};
use crate::convert::ConvertError;


const HSBW_ESCAPE: u8 = 12;
const HSBW_OP: u8 = 34;
const ENDCHAR: u8 = 14;

/// One parsed Type 2 operation: the operator name is the CFF byte meaning
/// the Ruby converter dispatches on.
#[derive(Clone, Debug)]
struct Op {
    name: OpName,
    operands: Vec<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OpName {
    Hstem,
    Vstem,
    Hstemhm,
    Vstemhm,
    Hintmask,
    Rmoveto,
    Hmoveto,
    Vmoveto,
    Rlineto,
    Hlineto,
    Vlineto,
    Rrcurveto,
    Hhcurveto,
    Hvcurveto,
    Vhcurveto,
    Rcurveline,
    Rlinecurve,
    Callsubr,
    Return,
    Endchar,
    Other,
}

const COORD_OPS: &[OpName] = &[
    OpName::Rmoveto,
    OpName::Hmoveto,
    OpName::Vmoveto,
    OpName::Rlineto,
    OpName::Hlineto,
    OpName::Vlineto,
    OpName::Rrcurveto,
    OpName::Hhcurveto,
    OpName::Hvcurveto,
    OpName::Vhcurveto,
    OpName::Rcurveline,
    OpName::Rlinecurve,
    OpName::Hstem,
    OpName::Vstem,
    OpName::Hstemhm,
    OpName::Vstemhm,
];

fn name_of(op: u32) -> OpName {
    match op {
        1 => OpName::Hstem,
        3 => OpName::Vstem,
        18 => OpName::Hstemhm,
        19 | 20 => OpName::Hintmask,
        21 => OpName::Rmoveto,
        22 => OpName::Hmoveto,
        4 => OpName::Vmoveto,
        5 => OpName::Rlineto,
        6 => OpName::Hlineto,
        7 => OpName::Vlineto,
        8 => OpName::Rrcurveto,
        27 => OpName::Hhcurveto,
        31 => OpName::Hvcurveto,
        30 => OpName::Vhcurveto,
        24 => OpName::Rcurveline,
        25 => OpName::Rlinecurve,
        10 => OpName::Callsubr,
        11 => OpName::Return,
        14 => OpName::Endchar,
        _ => OpName::Other,
    }
}

/// The Type 2 charstring walk (Ruby's `CharStringParser#parse`): operands
/// accumulate until an operator byte; stems count as they arrive; hintmask
/// consumes `(stems + 7) / 8` mask bytes.
fn parse_charstring(data: &[u8]) -> Vec<Op> {
    let mut ops = Vec::new();
    let mut operands: Vec<i64> = Vec::new();
    let mut stems: usize = 0;
    let mut i = 0usize;
    while i < data.len() {
        let b0 = data[i];
        i += 1;
        if b0 <= 31 && b0 != 28 {
            let name = if b0 == 12 {
                let Some(&b1) = data.get(i) else { break };
                i += 1;
                // Escape operators are outside the converter's vocabulary;
                // they carry no mask or stems.
                OpName::Other
            } else {
                name_of(u32::from(b0))
            };
            if matches!(name, OpName::Hstem | OpName::Vstem | OpName::Hstemhm | OpName::Vstemhm) {
                stems += operands.len() / 2;
            }
            if name == OpName::Hintmask {
                let mask = (stems + 7) / 8;
                i += mask;
            }
            ops.push(Op { name, operands: std::mem::take(&mut operands) });
        } else {
            let Some((value, consumed)) = read_number(&data[i.saturating_sub(1)..]) else {
                break;
            };
            operands.push(value);
            i += consumed - 1;
        }
    }
    ops
}

/// CFF number encodings (byte offsets relative to the first byte).
fn read_number(data: &[u8]) -> Option<(i64, usize)> {
    match data.first()? {
        28 if data.len() >= 3 => {
            Some((i16::from_be_bytes([data[1], data[2]]) as i64, 3))
        }
        29 if data.len() >= 5 => {
            Some((i32::from_be_bytes([data[1], data[2], data[3], data[4]]) as i64, 5))
        }
        32..=246 => Some((i64::from(data[0]) - 139, 1)),
        247..=250 if data.len() >= 2 => {
            Some((i64::from(data[0] - 247) * 256 + i64::from(data[1]) + 108, 2))
        }
        251..=254 if data.len() >= 2 => {
            Some(-(i64::from(data[0] - 251) * 256 + i64::from(data[1]) + 108))
                .map(|v| (v, 2))
        }
        255 if data.len() >= 5 => {
            Some((i32::from_be_bytes([data[1], data[2], data[3], data[4]]) as i64, 5))
        }
        _ => None,
    }
}

/// Ruby's `Rational#round` (half away from zero) over num/den.
fn rational_round(num: i64, den: i64) -> i64 {
    if den <= 0 {
        return num.div_euclid(den.max(1));
    }
    let a = num.abs() * 2 + den;
    let q = a / (den * 2);
    if num >= 0 { q } else { -q }
}

/// Multiply every coordinate operand by the scale factor, rounding to whole
/// font units (Ruby's `apply_scale!`).
fn apply_scale(ops: &mut [Op], num: i64, den: i64) {
    for op in ops.iter_mut() {
        op.operands = op.operands.iter().map(|v| rational_round(v * num, den)).collect();
    }
}

/// The odd-stack width before the first move (Ruby's `extract_width`).
fn extract_width(ops: &[Op], default_width: i64) -> i64 {
    if ops.is_empty() {
        return default_width;
    }
    let Some(idx) = ops.iter().position(|op| {
        matches!(
            op.name,
            OpName::Rmoveto | OpName::Hmoveto | OpName::Vmoveto | OpName::Rcurveline | OpName::Rrcurveto
        )
    }) else {
        return default_width;
    };
    let before: usize = ops[..idx].iter().map(|op| op.operands.len()).sum();
    if before % 2 == 1 {
        if let Some(first) = ops[idx].operands.first() {
            return *first;
        }
    }
    default_width
}

/// The Type 1 byte conversion (Ruby's `convert_operations`).
fn convert_operations(ops: &[Op], width: i64, scaled: bool) -> Vec<u8> {
    let mut out = encode_number(0);
    out.extend_from_slice(&encode_number(i32::try_from(width).unwrap_or(0)));
    out.push(HSBW_ESCAPE);
    out.push(HSBW_OP);

    let mut first_move = true;
    let mut skip_first_operand = false;
    if width != 0 && !ops.is_empty() {
        if let Some(idx) = ops.iter().position(|op| {
            matches!(
                op.name,
                OpName::Rmoveto | OpName::Hmoveto | OpName::Vmoveto | OpName::Rcurveline | OpName::Rrcurveto
            )
        }) {
            let before: usize = ops[..idx].iter().map(|op| op.operands.len()).sum();
            skip_first_operand = before % 2 == 1;
        }
    }
    let _ = scaled;

    for op in ops {
        if COORD_OPS.contains(&op.name) && op.operands.is_empty() {
            continue;
        }
        match op.name {
            OpName::Hstem | OpName::Vstem | OpName::Hstemhm | OpName::Vstemhm => {
                for val in &op.operands {
                    out.extend_from_slice(&encode_number(*val as i32));
                }
                out.push(stem_op_code(op.name));
            }
            OpName::Rmoveto => {
                let mut operands = op.operands.as_slice();
                if first_move && skip_first_operand && !operands.is_empty() {
                    operands = &operands[1..];
                    skip_first_operand = false;
                }
                if operands.len() >= 2 {
                    out.extend_from_slice(&encode_number(operands[0] as i32));
                    out.extend_from_slice(&encode_number(operands[1] as i32));
                    out.push(21);
                } else if operands.len() == 1 {
                    out.extend_from_slice(&encode_number(operands[0] as i32));
                    out.push(4);
                }
                first_move = false;
            }
            OpName::Hmoveto => {
                let mut operands = op.operands.as_slice();
                if first_move && skip_first_operand && operands.len() > 1 {
                    operands = &operands[..1];
                    skip_first_operand = false;
                }
                if let Some(&dx) = operands.first() {
                    out.extend_from_slice(&encode_number(dx as i32));
                }
                out.push(22);
                first_move = false;
            }
            OpName::Vmoveto => {
                if let Some(&dy) = op.operands.first() {
                    out.extend_from_slice(&encode_number(dy as i32));
                }
                out.push(4);
                first_move = false;
            }
            OpName::Rlineto => {
                if op.operands.len() >= 2 {
                    let (dx, dy) = (op.operands[0], op.operands[1]);
                    out.extend_from_slice(&encode_number(dx as i32));
                    out.extend_from_slice(&encode_number(dy as i32));
                    out.push(5);
                }
                first_move = false;
            }
            OpName::Hlineto => {
                if let Some(&dx) = op.operands.first() {
                    out.extend_from_slice(&encode_number(dx as i32));
                }
                out.push(6);
                first_move = false;
            }
            OpName::Vlineto => {
                if let Some(&dy) = op.operands.first() {
                    out.extend_from_slice(&encode_number(dy as i32));
                }
                out.push(7);
                first_move = false;
            }
            OpName::Rrcurveto => {
                if op.operands.len() >= 6 {
                    for val in &op.operands[..6] {
                        out.extend_from_slice(&encode_number(*val as i32));
                    }
                    out.push(8);
                }
                first_move = false;
            }
            OpName::Hhcurveto | OpName::Hvcurveto | OpName::Vhcurveto | OpName::Rcurveline
            | OpName::Rlinecurve => {
                for val in &op.operands {
                    out.extend_from_slice(&encode_number(*val as i32));
                }
                out.push(flex_op_code(op.name));
                first_move = false;
            }
            OpName::Callsubr | OpName::Return | OpName::Endchar => {
                out.push(subr_op_code(op.name));
                first_move = false;
            }
            OpName::Hintmask | OpName::Other => {
                first_move = false;
            }
        }
    }
    out.push(ENDCHAR);
    out
}

fn stem_op_code(name: OpName) -> u8 {
    match name {
        OpName::Hstem => 1,
        OpName::Vstem => 3,
        OpName::Hstemhm => 18,
        _ => 23,
    }
}

fn flex_op_code(name: OpName) -> u8 {
    match name {
        OpName::Hhcurveto => 27,
        OpName::Hvcurveto => 31,
        OpName::Vhcurveto => 30,
        OpName::Rcurveline => 24,
        _ => 25,
    }
}

fn subr_op_code(name: OpName) -> u8 {
    match name {
        OpName::Callsubr => 10,
        OpName::Return => 11,
        _ => 14,
    }
}

/// The otf → type1 conversion: the PFB bytes for an OTF source, mirroring
/// Ruby's `Type1Converter#convert_otf_to_type1` + `build_type1_data`.
pub fn otf_to_type1(_tables: &[( [u8; 4], Vec<u8>)]) -> Result<Vec<u8>, ConvertError> {
    let _ = Pfb {
        psname: "",
        version: "1.0",
        fontinfo: Vec::new(),
        bbox: [0; 4],
        private_entries: Vec::new(),
        charstrings: Vec::new(),
    };
    Err(ConvertError::transform("otf->type1 is not wired yet"))
}
