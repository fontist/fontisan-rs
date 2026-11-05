//! Port of `Tables::Cff::CharStringBuilder`, `IndexBuilder`, `DictBuilder`
//! and `Converters::CffTableBuilder` (byte-literal, quirks included: the
//! offset passes, the trailing empty local-subr INDEX, the Name INDEX).

use super::glyf::Cmd;

/// An outline command after `Outline#to_cff_commands` rounding.
enum Cff {
    Move(i64, i64),
    Line(i64, i64),
    Curve([i64; 6]),
}

fn rnd(v: f64) -> i64 {
    v.round() as i64
}

/// `Outline#to_cff_commands` (close_path already dropped).
fn to_cff(cmds: &[Cmd]) -> Vec<Cff> {
    let mut out: Vec<Cff> = Vec::new();
    for c in cmds {
        match *c {
            Cmd::Move(x, y) => out.push(Cff::Move(rnd(x), rnd(y))),
            Cmd::Line(x, y) => out.push(Cff::Line(rnd(x), rnd(y))),
            Cmd::Quad(cx, cy, x, y) => {
                let (px, py) = out
                    .iter()
                    .next_back()
                    .map(|c| match *c {
                        Cff::Move(x, y) | Cff::Line(x, y) => (x, y),
                        Cff::Curve(v) => (v[4], v[5]),
                    })
                    .unwrap_or((0, 0));
                let t = 2.0 / 3.0;
                let (pxf, pyf) = (px as f64, py as f64);
                out.push(Cff::Curve([
                    rnd(pxf + t * (cx - pxf)),
                    rnd(pyf + t * (cy - pyf)),
                    rnd(x + t * (cx - x)),
                    rnd(y + t * (cy - y)),
                    rnd(x),
                    rnd(y),
                ]));
            }
        }
    }
    out
}

fn number(o: &mut Vec<u8>, v: i64) {
    if (-107..=107).contains(&v) {
        o.push((v + 139) as u8);
    } else if (108..=1131).contains(&v) {
        let a = v - 108;
        o.push((247 + a / 256) as u8);
        o.push((a % 256) as u8);
    } else if (-1131..=-108).contains(&v) {
        let a = -v - 108;
        o.push((251 + a / 256) as u8);
        o.push((a % 256) as u8);
    } else if (-32768..=32767).contains(&v) {
        o.push(28);
        o.extend_from_slice(&(v as i16).to_be_bytes());
    } else {
        o.push(255);
        let fixed = ((v as f64) * 65536.0).round() as i64;
        o.extend_from_slice(&(fixed as i32).to_be_bytes());
    }
}

/// `CharStringBuilder#build` / `#build_empty`.
pub(super) fn charstring(cmds: &[Cmd]) -> Vec<u8> {
    let mut o = Vec::new();
    let (mut cx, mut cy) = (0i64, 0i64);
    let mut first = true;
    for c in to_cff(cmds) {
        match c {
            Cff::Move(x, y) => {
                let (dx, dy) = (x - cx, y - cy);
                if first && dx == 0 {
                    number(&mut o, dy);
                    o.push(4);
                } else if first && dy == 0 {
                    number(&mut o, dx);
                    o.push(22);
                } else {
                    number(&mut o, dx);
                    number(&mut o, dy);
                    o.push(21);
                }
                first = false;
                cx = x;
                cy = y;
            }
            Cff::Line(x, y) => {
                number(&mut o, x - cx);
                number(&mut o, y - cy);
                o.push(5);
                cx = x;
                cy = y;
            }
            Cff::Curve([x1, y1, x2, y2, x, y]) => {
                for d in [x1 - cx, y1 - cy, x2 - x1, y2 - y1, x - x2, y - y2] {
                    number(&mut o, d);
                }
                o.push(8);
                cx = x;
                cy = y;
            }
        }
    }
    o.push(14);
    o
}

/// `IndexBuilder.build`.
pub(super) fn index(items: &[Vec<u8>]) -> Vec<u8> {
    if items.is_empty() {
        return vec![0, 0];
    }
    let data_size: usize = items.iter().map(Vec::len).sum();
    let max = data_size + 1;
    let off_size: usize = if max <= 0xFF {
        1
    } else if max <= 0xFFFF {
        2
    } else if max <= 0xFF_FFFF {
        3
    } else {
        4
    };
    let mut o = Vec::new();
    o.extend_from_slice(&(items.len() as u16).to_be_bytes());
    o.push(off_size as u8);
    let mut off = 1usize;
    let put =
        |o: &mut Vec<u8>, v: usize| o.extend_from_slice(&(v as u32).to_be_bytes()[4 - off_size..]);
    put(&mut o, off);
    for it in items {
        off += it.len();
        put(&mut o, off);
    }
    for it in items {
        o.extend_from_slice(it);
    }
    o
}

/// `DictBuilder.write_integer`.
fn dict_int(o: &mut Vec<u8>, v: i64) {
    if (-107..=107).contains(&v) {
        o.push((v + 139) as u8);
    } else if (108..=1131).contains(&v) {
        let a = v - 108;
        o.push((247 + a / 256) as u8);
        o.push((a % 256) as u8);
    } else if (-1131..=-108).contains(&v) {
        let a = -v - 108;
        o.push((251 + a / 256) as u8);
        o.push((a % 256) as u8);
    } else if (-32768..=32767).contains(&v) {
        o.push(28);
        o.extend_from_slice(&(v as i16).to_be_bytes());
    } else {
        o.push(29);
        o.extend_from_slice(&(v as i32).to_be_bytes());
    }
}

fn top_dict(charstrings: usize, private: Option<(usize, usize)>) -> Vec<u8> {
    let mut o = Vec::new();
    dict_int(&mut o, 0);
    o.push(15); // charset
    dict_int(&mut o, 0);
    o.push(16); // encoding
    dict_int(&mut o, charstrings as i64);
    o.push(17);
    if let Some((size, off)) = private {
        dict_int(&mut o, size as i64);
        dict_int(&mut o, off as i64);
        o.push(18);
    }
    o
}

/// `CffTableBuilder#build_cff_table` with no local subrs.
pub(super) fn cff_table(charstrings: &[Vec<u8>], font_name: Vec<u8>) -> Vec<u8> {
    let name_idx = index(&[font_name]);
    let string_idx = index(&[]);
    let global_idx = index(&[]);
    let cs_idx = index(charstrings);
    let local_idx = index(&[]);
    let mut private = Vec::new();
    dict_int(&mut private, 1000);
    private.push(20);
    dict_int(&mut private, 0);
    private.push(21);

    let top_start = 4 + name_idx.len();
    // pass 1
    let string_start = top_start + 100;
    let cs_off = string_start + string_idx.len() + global_idx.len();
    let top1 = index(&[top_dict(cs_off, None)]);
    // pass 2
    let cs_off = top_start + top1.len() + string_idx.len() + global_idx.len();
    let priv_off = cs_off + cs_idx.len();
    let top2 = index(&[top_dict(cs_off, Some((private.len(), priv_off)))]);
    // pass 3 (offsets from pass 2's size, as Ruby does)
    let cs_off = top_start + top2.len() + string_idx.len() + global_idx.len();
    let priv_off = cs_off + cs_idx.len();
    let top3 = index(&[top_dict(cs_off, Some((private.len(), priv_off)))]);

    let mut out = vec![1, 0, 4, 4];
    for part in [
        &name_idx,
        &top3,
        &string_idx,
        &global_idx,
        &cs_idx,
        &private,
        &local_idx,
    ] {
        out.extend_from_slice(part);
    }
    out
}
