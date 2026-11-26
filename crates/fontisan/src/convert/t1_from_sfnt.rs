//! OTF → Type 1 (fontisan#192 gap 4, stage 2): the op-level conversion the
//! Ruby `CffToType1Converter` performs - parse the Type 2 charstring into
//! operations (stems counted as they arrive, hint masks consumed), rescale
//! coordinates to 1000 upm, extract the odd-stack width, and emit the Type 1
//! bytecode (hsbw prefix, walk, endchar) - then assemble the canonical PFB
//! with the source's names, FontInfo, Private entries and scaled FontBBox.

use crate::convert::ConvertError;
use crate::convert::t1write::Pfb;

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
                if data.get(i).is_none() {
                    break;
                }
                i += 1;
                // Escape operators are outside the converter's vocabulary;
                // they carry no mask or stems.
                OpName::Other
            } else {
                name_of(u32::from(b0))
            };
            if matches!(
                name,
                OpName::Hstem | OpName::Vstem | OpName::Hstemhm | OpName::Vstemhm
            ) {
                stems += operands.len() / 2;
            }
            if name == OpName::Hintmask {
                let mask = stems.div_ceil(8);
                i += mask;
            }
            ops.push(Op {
                name,
                operands: std::mem::take(&mut operands),
            });
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
        28 if data.len() >= 3 => Some((i16::from_be_bytes([data[1], data[2]]) as i64, 3)),
        29 if data.len() >= 5 => Some((
            i32::from_be_bytes([data[1], data[2], data[3], data[4]]) as i64,
            5,
        )),
        32..=246 => Some((i64::from(data[0]) - 139, 1)),
        247..=250 if data.len() >= 2 => {
            Some((i64::from(data[0] - 247) * 256 + i64::from(data[1]) + 108, 2))
        }
        251..=254 if data.len() >= 2 => {
            Some(-(i64::from(data[0] - 251) * 256 + i64::from(data[1]) + 108)).map(|v| (v, 2))
        }
        255 if data.len() >= 5 => Some((
            i32::from_be_bytes([data[1], data[2], data[3], data[4]]) as i64,
            5,
        )),
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
        op.operands = op
            .operands
            .iter()
            .map(|v| rational_round(v * num, den))
            .collect();
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
            OpName::Rmoveto
                | OpName::Hmoveto
                | OpName::Vmoveto
                | OpName::Rcurveline
                | OpName::Rrcurveto
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

/// Ruby's `CffToType1Converter#encode_number`: one cutoff at 107, then the
/// 255 escape - no 108..1131 two-byte form (that is the OTHER path's
/// encoder, `TTFToType1Converter#encode_number`, which t1write carries).
fn cff_encode_number(num: i64) -> Vec<u8> {
    if (-107..=107).contains(&num) {
        vec![(num + 139) as u8]
    } else {
        let num = if num < 0 { num + 32768 } else { num };
        vec![255, (num % 256) as u8, (num >> 8) as u8]
    }
}

/// The Type 1 byte conversion (Ruby's `convert_operations`).
fn convert_operations(ops: &[Op], width: i64) -> Vec<u8> {
    let mut out = cff_encode_number(0);
    out.extend_from_slice(&cff_encode_number(width));
    out.push(HSBW_ESCAPE);
    out.push(HSBW_OP);

    let mut first_move = true;
    let mut skip_first_operand = false;
    if !ops.is_empty() {
        if let Some(idx) = ops.iter().position(|op| {
            matches!(
                op.name,
                OpName::Rmoveto
                    | OpName::Hmoveto
                    | OpName::Vmoveto
                    | OpName::Rcurveline
                    | OpName::Rrcurveto
            )
        }) {
            let before: usize = ops[..idx].iter().map(|op| op.operands.len()).sum();
            skip_first_operand = before % 2 == 1;
        }
    }

    for op in ops {
        if COORD_OPS.contains(&op.name) && op.operands.is_empty() {
            continue;
        }
        match op.name {
            OpName::Hstem | OpName::Vstem | OpName::Hstemhm | OpName::Vstemhm => {
                for val in &op.operands {
                    out.extend_from_slice(&cff_encode_number(*val));
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
                    out.extend_from_slice(&cff_encode_number(operands[0]));
                    out.extend_from_slice(&cff_encode_number(operands[1]));
                    out.push(21);
                } else if operands.len() == 1 {
                    out.extend_from_slice(&cff_encode_number(operands[0]));
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
                    out.extend_from_slice(&cff_encode_number(dx));
                }
                out.push(22);
                first_move = false;
            }
            OpName::Vmoveto => {
                if let Some(&dy) = op.operands.first() {
                    out.extend_from_slice(&cff_encode_number(dy));
                }
                out.push(4);
                first_move = false;
            }
            OpName::Rlineto => {
                if op.operands.len() >= 2 {
                    let (dx, dy) = (op.operands[0], op.operands[1]);
                    out.extend_from_slice(&cff_encode_number(dx));
                    out.extend_from_slice(&cff_encode_number(dy));
                    out.push(5);
                }
                first_move = false;
            }
            OpName::Hlineto => {
                if let Some(&dx) = op.operands.first() {
                    out.extend_from_slice(&cff_encode_number(dx));
                }
                out.push(6);
                first_move = false;
            }
            OpName::Vlineto => {
                if let Some(&dy) = op.operands.first() {
                    out.extend_from_slice(&cff_encode_number(dy));
                }
                out.push(7);
                first_move = false;
            }
            OpName::Rrcurveto => {
                if op.operands.len() >= 6 {
                    for val in &op.operands[..6] {
                        out.extend_from_slice(&cff_encode_number(*val));
                    }
                    out.push(8);
                }
                first_move = false;
            }
            OpName::Hhcurveto
            | OpName::Hvcurveto
            | OpName::Vhcurveto
            | OpName::Rcurveline
            | OpName::Rlinecurve => {
                for val in &op.operands {
                    out.extend_from_slice(&cff_encode_number(*val));
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
pub fn otf_to_type1(tables: &[([u8; 4], Vec<u8>)]) -> Result<Vec<u8>, ConvertError> {
    let tag = |t: &[u8]| {
        tables
            .iter()
            .find(|(tag, _)| tag.as_slice() == t)
            .map(|(_, data)| data.as_slice())
    };
    let name_data = tag(b"name");
    let head_data = tag(b"head");
    let post_data = tag(b"post");
    let os2_data = tag(b"OS/2");
    let Some(cff_data) = tag(b"CFF ") else {
        return Err(ConvertError::transform("CFF table not found"));
    };

    let name = name_data.and_then(|d| crate::sfnt::tables::Name::parse(d).ok());
    let head = head_data.and_then(|d| crate::sfnt::tables::Head::parse(d).ok());
    let post = post_data.and_then(|d| crate::sfnt::tables::Post::parse(d).ok());
    let os2 = os2_data.and_then(|d| crate::sfnt::tables::Os2::parse(d).ok());
    let cff = crate::cffread::parse(cff_data)
        .map_err(|e| ConvertError::transform(format!("Failed to parse CFF table: {e}")))?;

    let psname = name
        .as_ref()
        .and_then(|n| n.english_name(6))
        .unwrap_or_else(|| "Untitled".to_string());
    let version = name
        .as_ref()
        .and_then(|n| n.english_name(5))
        .unwrap_or_else(|| "1.0".to_string());

    // fontinfo_of: version, FullName, FamilyName, [Copyright], ItalicAngle,
    // isFixedPitch, Weight - the rendered lines the PFB header writes.
    let mut fontinfo = vec![format!("/version ({version}) def")];
    let name_ref = name.as_ref();
    fontinfo.push(format!(
        "/FullName ({}) def",
        name_ref
            .and_then(|n| n.english_name(4))
            .unwrap_or_else(|| psname.clone())
    ));
    fontinfo.push(format!(
        "/FamilyName ({}) def",
        name_ref
            .and_then(|n| n.english_name(1))
            .unwrap_or_else(|| psname.clone())
    ));
    if let Some(copyright) = name_ref.and_then(|n| n.english_name(0)) {
        fontinfo.push(format!("/Copyright ({copyright}) def"));
    }
    let italic_angle = post
        .as_ref()
        .map_or(0.0, |p| f64::from(p.italic_angle_raw) / 65536.0);
    fontinfo.push(format!("/ItalicAngle {italic_angle:.6} def"));
    let fixed_pitch = post.as_ref().is_some_and(|p| p.is_fixed_pitch != 0);
    fontinfo.push(format!("/isFixedPitch {fixed_pitch} def"));
    let weight_class = os2.as_ref().map_or(0, |o| o.us_weight_class);
    let weight = if weight_class == 0 {
        "Regular".to_string()
    } else {
        WEIGHT_NAMES[((weight_class / 100).clamp(1, 9) - 1) as usize].to_string()
    };
    fontinfo.push(format!("/Weight ({weight}) def"));

    // bbox_of: the head box scaled to 1000 upm.
    let upm = i64::from(head.as_ref().map_or(0, |h| h.units_per_em));
    let scale = if upm > 0 { (1000, upm) } else { (1, 1) };
    let scaled = |v: i64| rational_round(v * scale.0, scale.1) as i32;
    let bbox = head.as_ref().map_or([0; 4], |h| {
        [
            scaled(i64::from(h.x_min)),
            scaled(i64::from(h.y_min)),
            scaled(i64::from(h.x_max)),
            scaled(i64::from(h.y_max)),
        ]
    });

    // private_entries_from: the CFF Private DICT's hint fields carried over.
    let op_value = |op: u32| -> Option<String> {
        let v = cff.private_ops.iter().find(|(o, _)| *o == op)?.1.as_slice();
        Some(match v.len() {
            1 => v[0].to_string(),
            _ => format!(
                "[{}]",
                v.iter()
                    .map(|x| x.to_string())
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
        })
    };
    let mut private_entries: Vec<(&str, String)> = vec![
        ("BlueValues", "[0 0]".to_string()),
        ("password", "5839".to_string()),
        ("MinFeature", "{16 16}".to_string()),
    ];
    if let Some(blue) = op_value(6) {
        private_entries[0].1 = blue;
    }
    if let Some(hw) = op_value(10) {
        private_entries.push(("StdHW", hw));
    }
    if let Some(vw) = op_value(11) {
        private_entries.push(("StdVW", vw));
    }
    private_entries.push(("ForceBold", "false".to_string()));

    // The converter's widths from the Private DICT (ops 21/20), scaled.
    let scalar = |op: u32| -> i64 {
        cff.private_ops
            .iter()
            .find(|(o, _)| *o == op)
            .and_then(|(_, v)| v.first())
            .copied()
            .unwrap_or(0)
    };
    let default = rational_round(scalar(20) * scale.0, scale.1);

    // Glyph names: the charset SIDs (standard strings + the font's String
    // INDEX), Ruby's dangling-SID and duplicate fallbacks, post table next.
    let mut names: Vec<String> = Vec::with_capacity(cff.charstrings.len());
    for gid in 0..cff.charstrings.len() {
        let mut name = charset_name(&cff, gid)
            .or_else(|| {
                post.as_ref()
                    .and_then(|p| p.glyph_names().get(gid).cloned())
            })
            .unwrap_or_else(|| format!("glyph{gid}"));
        if gid > 0 && (name == ".notdef" || names.contains(&name)) {
            name = format!("glyph{gid}");
        }
        names.push(name);
    }

    let mut charstrings = Vec::with_capacity(cff.charstrings.len());
    for (gid, cs) in cff.charstrings.iter().enumerate() {
        let mut ops = parse_charstring(cs);
        apply_scale(&mut ops, scale.0, scale.1);
        let width = extract_width(&ops, default);
        charstrings.push((names[gid].as_str(), convert_operations(&ops, width)));
    }

    Pfb {
        psname: &psname,
        version: &version,
        fontinfo,
        bbox,
        private_entries,
        charstrings,
    }
    .write()
}

/// Ruby's `Charset#glyph_name` over the parsed SIDs: standard strings for
/// SID 0-390, the font's String INDEX above, `.notdef` for dangling SIDs,
/// nil beyond the charset's length.
fn charset_name(cff: &crate::cffread::Cff, gid: usize) -> Option<String> {
    let sids = cff.charset_sids.as_ref()?;
    let sid = (*sids.get(gid)?)?;
    let sid = usize::try_from(sid).ok()?;
    if sid <= 390 {
        Some(crate::sfnt::standard_strings::STANDARD[sid].to_string())
    } else {
        cff.strings
            .get(sid - 391)
            .map(|b| String::from_utf8_lossy(b).to_string())
            .or_else(|| Some(".notdef".to_string()))
    }
}

/// Ruby's `PFBGenerator::WEIGHT_NAMES`.
const WEIGHT_NAMES: [&str; 9] = [
    "Thin",
    "ExtraLight",
    "Light",
    "Regular",
    "Medium",
    "SemiBold",
    "Bold",
    "ExtraBold",
    "Black",
];
