//! Minimal CFF1 reader for the o2t port: what Ruby's `Tables::Cff` does for
//! `charstring_for_glyph(i)` on font 0 (CharStrings, Private DICT, Subrs).

type R<T> = Result<T, String>;

pub(super) struct Index<'a> {
    pub items: Vec<&'a [u8]>,
    pub end: usize,
}

fn take(data: &[u8], pos: usize, n: usize) -> R<&[u8]> {
    if n == 0 {
        return Ok(&[]);
    }
    data.get(pos..)
        .and_then(|r| r.get(..n))
        .ok_or_else(|| "Unexpected end of INDEX data".to_string())
}

pub(super) fn index(data: &[u8], start: usize) -> R<Index<'_>> {
    let count = usize::from(u16::from_be_bytes(
        take(data, start, 2)?.try_into().unwrap(),
    ));
    if count == 0 {
        return Ok(Index {
            items: vec![],
            end: start + 2,
        });
    }
    let off_size = usize::from(take(data, start + 2, 1)?[0]);
    if !(1..=4).contains(&off_size) {
        return Err(format!("Invalid INDEX offSize: {off_size} (must be 1-4)"));
    }
    let mut offsets = Vec::with_capacity(count + 1);
    for i in 0..=count {
        let b = take(data, start + 3 + i * off_size, off_size)?;
        offsets.push(b.iter().fold(0usize, |a, &x| (a << 8) | usize::from(x)));
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
    let data_start = start + 3 + (count + 1) * off_size;
    let data_size = offsets[count] - 1;
    take(data, data_start, data_size)?;
    Ok(Index {
        items: offsets
            .windows(2)
            .map(|w| &data[data_start + w[0] - 1..data_start + w[1] - 1])
            .collect(),
        end: data_start + data_size,
    })
}

/// DICT entries `(operator, operands)`; a later duplicate replaces an earlier
/// one (Ruby hash). Real operands are kept as 0 (never used here).
fn dict(d: &[u8]) -> R<Vec<(u32, Vec<i64>)>> {
    let eof = || "Unexpected end of DICT".to_string();
    let mut out: Vec<(u32, Vec<i64>)> = vec![];
    let mut stack: Vec<i64> = vec![];
    let mut i = 0;
    while i < d.len() {
        let b = d[i];
        i += 1;
        if b <= 21 {
            let op = if b == 12 {
                let b2 = *d.get(i).ok_or_else(eof)?;
                i += 1;
                1200 + u32::from(b2)
            } else {
                u32::from(b)
            };
            let ops = std::mem::take(&mut stack);
            match out.iter_mut().find(|(o, _)| *o == op) {
                Some(slot) => slot.1 = ops,
                None => out.push((op, ops)),
            }
            continue;
        }
        let v = match b {
            28 => {
                let s = d.get(i..i + 2).ok_or_else(eof)?;
                i += 2;
                i64::from(i16::from_be_bytes([s[0], s[1]]))
            }
            29 => {
                let s = d.get(i..i + 4).ok_or_else(eof)?;
                i += 4;
                i64::from(i32::from_be_bytes([s[0], s[1], s[2], s[3]]))
            }
            30 => {
                loop {
                    let n = *d.get(i).ok_or_else(eof)?;
                    i += 1;
                    if n >> 4 == 0xF || n & 0xF == 0xF {
                        break;
                    }
                }
                0
            }
            32..=246 => i64::from(b) - 139,
            247..=250 => {
                let s = *d.get(i).ok_or_else(eof)?;
                i += 1;
                (i64::from(b) - 247) * 256 + i64::from(s) + 108
            }
            251..=254 => {
                let s = *d.get(i).ok_or_else(eof)?;
                i += 1;
                -(i64::from(b) - 251) * 256 - i64::from(s) - 108
            }
            _ => return Err(format!("Invalid DICT operand byte: {b}")),
        };
        stack.push(v);
    }
    Ok(out)
}

/// Ruby stores one operand as a scalar, several as an array.
fn get(d: &[(u32, Vec<i64>)], op: u32) -> Option<&Vec<i64>> {
    d.iter().find(|(o, _)| *o == op).map(|(_, v)| v)
}

pub(super) struct Cff<'a> {
    pub charstrings: Vec<&'a [u8]>,
    pub global_subrs: Vec<&'a [u8]>,
    /// None when Ruby's `charstring_for_glyph` returns nil (no Private DICT).
    pub local_subrs: Option<Vec<&'a [u8]>>,
    pub has_private: bool,
}

pub(super) fn parse(data: &[u8]) -> R<Cff<'_>> {
    let h = data.get(..4).ok_or("Unexpected end of data")?;
    let (major, minor, hdr_size, off_size) = (h[0], h[1], h[2], h[3]);
    if !(major == 1 || major == 2) || minor != 0 || hdr_size < 4 || !(1..=4).contains(&off_size) {
        return Err(format!(
            "Invalid CFF header: version={major}.{minor}, hdr_size={hdr_size}, off_size={off_size}"
        ));
    }
    let names = index(data, usize::from(hdr_size))?;
    let tops = index(data, names.end)?;
    let strings = index(data, tops.end)?;
    let gsubrs = index(data, strings.end)?;
    let top = match tops.items.first() {
        Some(t) => dict(t)?,
        None => vec![],
    };
    let mut cff = Cff {
        charstrings: vec![],
        global_subrs: gsubrs.items,
        local_subrs: None,
        has_private: false,
    };
    let Some(cs) = get(&top, 17) else {
        return Ok(cff);
    };
    let off = *cs.first().ok_or("missing CharStrings offset")?;
    let off = usize::try_from(off).map_err(|_| "negative seek".to_string())?;
    cff.charstrings = index(data, off)?.items;
    // private_dict: [size, offset]; size <= 0 / offset past the end -> nil.
    let Some(p) = get(&top, 18) else {
        return Ok(cff);
    };
    if p.len() != 2 {
        return Err("undefined method 'negative?' for nil".into());
    }
    let (size, poff) = (p[0], p[1]);
    if size <= 0 || poff < 0 || poff as usize > data.len() {
        return Ok(cff);
    }
    let poff = poff as usize;
    let pdata = &data[poff..(poff + size as usize).min(data.len())];
    let pd = dict(pdata)?;
    cff.has_private = true;
    if let Some(s) = get(&pd, 19) {
        if s.len() != 1 {
            return Err("Failed to parse Local Subr INDEX".into());
        }
        let abs = poff as i64 + s[0];
        let abs = usize::try_from(abs).map_err(|_| "negative seek".to_string())?;
        cff.local_subrs = Some(index(data, abs)?.items);
    }
    Ok(cff)
}
