//! Port of Ruby's `Tables::Cff::CharString` interpreter, quirks included:
//! shift/pop based operand handling, `endchar` that does not stop, and
//! `hintmask`/`cntrmask` that skip their mask bytes only inside subroutines
//! (Ruby's `@io` is nil at top level, so the main charstring parses the mask
//! bytes as operators and numbers).

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Seg {
    Move(f64, f64),
    Line(f64, f64),
    Curve([f64; 6]),
}

type R<T> = Result<T, String>;

const MAX_DEPTH: usize = 64;
/// Total operators and operands one charstring (subroutines included) may
/// execute. Real glyphs need hundreds; this only stops exponential subr fan-out.
const MAX_OPS: usize = 1 << 20;

pub(crate) struct Interp<'a> {
    gsubrs: &'a [&'a [u8]],
    lsubrs: Option<&'a [&'a [u8]]>,
    gbias: usize,
    lbias: usize,
    stack: Vec<f64>,
    pub path: Vec<Seg>,
    x: f64,
    y: f64,
    width_set: bool,
    stems: usize,
    transient: Vec<f64>,
    depth: usize,
    ops: usize,
}

fn bias(n: Option<usize>) -> usize {
    match n {
        None => 0,
        Some(c) if c < 1240 => 107,
        Some(c) if c < 33900 => 1131,
        Some(_) => 32768,
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Op {
    Stem,
    Move(u8), // 0 rmoveto, 1 hmoveto, 2 vmoveto
    Rlineto,
    Hlineto,
    Vlineto,
    Rrcurveto,
    Hhcurveto,
    Vvcurveto,
    Hvcurveto,
    Vhcurveto,
    Rcurveline,
    Rlinecurve,
    Endchar,
    Mask,
    Callsubr,
    Callgsubr,
    Return,
    Add,
    Sub,
    Mul,
    Div,
    Neg,
    Abs,
    Sqrt,
    Drop,
    Exch,
    Dup,
    Put,
    Get,
    Flex(u8), // 0 hflex, 1 flex, 2 hflex1, 3 flex1
    Nop,      // named by Ruby but not executed (and, or, not, eq, ifelse, random, index, roll)
    Unknown,
}

fn decode(b: u8, second: u8) -> Op {
    if b == 12 {
        return match second {
            3 | 4 | 5 | 15 | 22 | 23 | 29 | 30 => Op::Nop,
            9 => Op::Abs,
            10 => Op::Add,
            11 => Op::Sub,
            12 => Op::Div,
            14 => Op::Neg,
            18 => Op::Drop,
            20 => Op::Put,
            21 => Op::Get,
            24 => Op::Mul,
            26 => Op::Sqrt,
            27 => Op::Dup,
            28 => Op::Exch,
            34 => Op::Flex(0),
            35 => Op::Flex(1),
            36 => Op::Flex(2),
            37 => Op::Flex(3),
            _ => Op::Unknown,
        };
    }
    match b {
        1 | 3 | 18 | 23 => Op::Stem,
        4 => Op::Move(2),
        5 => Op::Rlineto,
        6 => Op::Hlineto,
        7 => Op::Vlineto,
        8 => Op::Rrcurveto,
        10 => Op::Callsubr,
        11 => Op::Return,
        14 => Op::Endchar,
        19 | 20 => Op::Mask,
        21 => Op::Move(0),
        22 => Op::Move(1),
        24 => Op::Rcurveline,
        25 => Op::Rlinecurve,
        26 => Op::Vvcurveto,
        27 => Op::Hhcurveto,
        29 => Op::Callgsubr,
        30 => Op::Vhcurveto,
        31 => Op::Hvcurveto,
        _ => Op::Unknown,
    }
}

impl<'a> Interp<'a> {
    pub fn new(gsubrs: &'a [&'a [u8]], lsubrs: Option<&'a [&'a [u8]]>) -> Self {
        Self {
            gsubrs,
            lsubrs,
            gbias: bias(Some(gsubrs.len())),
            lbias: bias(lsubrs.map(<[_]>::len)),
            stack: vec![],
            path: vec![],
            x: 0.0,
            y: 0.0,
            width_set: false,
            stems: 0,
            transient: vec![],
            depth: 0,
            ops: 0,
        }
    }

    /// Interpret one charstring; the main one when `sub` is false.
    pub fn run_main(&mut self, data: &[u8]) -> R<()> {
        self.run(data, false)
    }

    fn run(&mut self, data: &[u8], sub: bool) -> R<()> {
        let mut pos = 0;
        let mut width_parsed = sub;
        while pos < data.len() {
            self.ops += 1;
            if self.ops > MAX_OPS {
                return Err("charstring operation budget exceeded".into());
            }
            let b = data[pos];
            pos += 1;
            if b <= 31 && b != 28 {
                let second = if b == 12 {
                    let s = *data.get(pos).ok_or("Unexpected end of CharString")?;
                    pos += 1;
                    s
                } else {
                    0
                };
                let op = decode(b, second);
                if op == Op::Return && sub {
                    break;
                }
                let moved = self.exec(op, width_parsed)?;
                if moved || op == Op::Stem {
                    // In a subroutine width_parsed is already true.
                    width_parsed = true;
                }
                if op == Op::Mask && sub {
                    pos = (pos + self.stems.div_ceil(8)).min(data.len());
                }
            } else {
                let (v, n) = number(data, pos - 1)?;
                pos = n;
                self.stack.push(v);
            }
        }
        Ok(())
    }

    fn pop(&mut self) -> R<f64> {
        self.stack.pop().ok_or_else(|| "nil".to_string())
    }

    fn shift(&mut self) -> R<f64> {
        if self.stack.is_empty() {
            return Err("nil can't be coerced into Float".into());
        }
        Ok(self.stack.remove(0))
    }

    fn width_for_move(&mut self, width_parsed: bool, expected: usize) {
        if width_parsed || self.width_set {
            return;
        }
        if self.stack.len() == expected + 1 {
            self.stack.remove(0);
        }
        self.width_set = true;
    }

    fn curve(&mut self, d: [f64; 6]) {
        let x1 = self.x + d[0];
        let y1 = self.y + d[1];
        let x2 = x1 + d[2];
        let y2 = y1 + d[3];
        self.x = x2 + d[4];
        self.y = y2 + d[5];
        self.path.push(Seg::Curve([x1, y1, x2, y2, self.x, self.y]));
    }

    fn line_to(&mut self) {
        self.path.push(Seg::Line(self.x, self.y));
    }

    /// Returns true for the operators Ruby reports as "width parsed".
    fn exec(&mut self, op: Op, width_parsed: bool) -> R<bool> {
        match op {
            Op::Move(kind) => {
                let expected = if kind == 0 { 2 } else { 1 };
                self.width_for_move(width_parsed, expected);
                if kind == 0 {
                    if self.stack.len() >= 2 {
                        let dy = self.pop()?;
                        let dx = self.pop()?;
                        self.x += dx;
                        self.y += dy;
                        self.path.push(Seg::Move(self.x, self.y));
                        self.stack.clear();
                    }
                } else if !self.stack.is_empty() {
                    let d = self.pop()?;
                    if kind == 1 {
                        self.x += d;
                    } else {
                        self.y += d;
                    }
                    self.path.push(Seg::Move(self.x, self.y));
                    self.stack.clear();
                }
                return Ok(true);
            }
            Op::Rlineto => {
                while self.stack.len() >= 2 {
                    let dx = self.shift()?;
                    let dy = self.shift()?;
                    self.x += dx;
                    self.y += dy;
                    self.line_to();
                }
                self.stack.clear();
            }
            Op::Hlineto | Op::Vlineto => {
                let mut horizontal = op == Op::Hlineto;
                while !self.stack.is_empty() {
                    let d = self.shift()?;
                    if horizontal {
                        self.x += d;
                    } else {
                        self.y += d;
                    }
                    self.line_to();
                    horizontal = !horizontal;
                }
                self.stack.clear();
            }
            Op::Rrcurveto => {
                while self.stack.len() >= 6 {
                    let mut d = [0.0; 6];
                    for v in &mut d {
                        *v = self.shift()?;
                    }
                    self.curve(d);
                }
                self.stack.clear();
            }
            Op::Hhcurveto => {
                if self.stack.len() % 2 == 1 {
                    let d = self.shift()?;
                    self.y += d;
                }
                while self.stack.len() >= 4 {
                    let (dx1, dx2, dy2, dx3) =
                        (self.shift()?, self.shift()?, self.shift()?, self.shift()?);
                    self.curve([dx1, 0.0, dx2, dy2, dx3, 0.0]);
                }
                self.stack.clear();
            }
            Op::Vvcurveto => {
                if self.stack.len() % 2 == 1 {
                    let d = self.shift()?;
                    self.x += d;
                }
                while self.stack.len() >= 4 {
                    let (dy1, dx2, dy2, dy3) =
                        (self.shift()?, self.shift()?, self.shift()?, self.shift()?);
                    self.curve([0.0, dy1, dx2, dy2, 0.0, dy3]);
                }
                self.stack.clear();
            }
            Op::Hvcurveto | Op::Vhcurveto => {
                let mut horizontal = op == Op::Hvcurveto;
                while self.stack.len() >= 4 {
                    let (a, b, c, d) = (self.shift()?, self.shift()?, self.shift()?, self.shift()?);
                    let last = if self.stack.len() == 1 {
                        self.shift()?
                    } else {
                        0.0
                    };
                    if horizontal {
                        // dx1 dx2 dy2 dy3 [dx3]
                        self.curve([a, 0.0, b, c, last, d]);
                    } else {
                        // dy1 dx2 dy2 dx3 [dy3]
                        self.curve([0.0, a, b, c, d, last]);
                    }
                    horizontal = !horizontal;
                }
                self.stack.clear();
            }
            Op::Rcurveline => {
                while self.stack.len() > 2 {
                    if self.stack.len() < 6 {
                        break;
                    }
                    let mut d = [0.0; 6];
                    for v in &mut d {
                        *v = self.shift()?;
                    }
                    self.curve(d);
                }
                if self.stack.len() == 2 {
                    let dx = self.shift()?;
                    let dy = self.shift()?;
                    self.x += dx;
                    self.y += dy;
                    self.line_to();
                }
                self.stack.clear();
            }
            Op::Rlinecurve => {
                while self.stack.len() > 6 {
                    let dx = self.shift()?;
                    let dy = self.shift()?;
                    self.x += dx;
                    self.y += dy;
                    self.line_to();
                }
                if self.stack.len() == 6 {
                    let mut d = [0.0; 6];
                    for v in &mut d {
                        *v = self.shift()?;
                    }
                    self.curve(d);
                }
                self.stack.clear();
            }
            Op::Endchar => self.stack.clear(),
            Op::Stem => {
                if !width_parsed && !self.width_set {
                    if self.stack.len() % 2 == 1 {
                        self.stack.remove(0);
                    }
                    self.width_set = true;
                }
                self.stems += self.stack.len() / 2;
                self.stack.clear();
            }
            Op::Mask => self.stack.clear(),
            Op::Callsubr | Op::Callgsubr => {
                if let Some(num) = self.stack.pop() {
                    let (list, b) = if op == Op::Callsubr {
                        (self.lsubrs, self.lbias)
                    } else {
                        (Some(self.gsubrs), self.gbias)
                    };
                    let idx = num + b as f64;
                    if let Some(list) = list {
                        if idx >= 0.0 && idx < list.len() as f64 {
                            let data: &'a [u8] = list[idx as usize];
                            if self.depth >= MAX_DEPTH {
                                return Err("stack level too deep".into());
                            }
                            self.depth += 1;
                            let r = self.run(data, true);
                            self.depth -= 1;
                            r?;
                        }
                    }
                }
            }
            Op::Return | Op::Nop => {}
            Op::Add | Op::Sub | Op::Mul => {
                if self.stack.len() >= 2 {
                    let b = self.pop()?;
                    let a = self.pop()?;
                    self.stack.push(match op {
                        Op::Add => a + b,
                        Op::Sub => a - b,
                        _ => a * b,
                    });
                }
            }
            Op::Div => {
                if self.stack.len() >= 2 {
                    let b = self.pop()?;
                    let a = self.pop()?;
                    if b != 0.0 {
                        self.stack.push(a / b);
                    }
                }
            }
            Op::Neg | Op::Abs => {
                if let Some(v) = self.stack.pop() {
                    self.stack.push(if op == Op::Neg { -v } else { v.abs() });
                }
            }
            Op::Sqrt => {
                if let Some(v) = self.stack.pop() {
                    if v >= 0.0 {
                        self.stack.push(v.sqrt());
                    }
                }
            }
            Op::Drop => {
                self.stack.pop();
            }
            Op::Exch => {
                let n = self.stack.len();
                if n < 2 {
                    return Err("exch needs two operands".into());
                }
                self.stack.swap(n - 1, n - 2);
            }
            Op::Dup => {
                let v = *self.stack.last().ok_or("dup on empty stack")?;
                self.stack.push(v);
            }
            Op::Put => {
                let v = self.pop()?;
                let i = self.pop()?;
                if i < 0.0 {
                    return Err("index out of range".into());
                }
                let i = i as usize;
                if self.transient.len() <= i {
                    self.transient.resize(i + 1, 0.0);
                }
                self.transient[i] = v;
            }
            Op::Get => {
                let i = self.pop()?;
                let v = if i >= 0.0 {
                    self.transient.get(i as usize).copied().unwrap_or(0.0)
                } else {
                    0.0
                };
                self.stack.push(v);
            }
            Op::Flex(kind) => {
                let need = [7usize, 12, 9, 11][kind as usize];
                if self.stack.len() < need {
                    return Err("nil can't be coerced into Float".into());
                }
                let s: Vec<f64> = self
                    .stack
                    .drain(
                        ..(if kind == 1 {
                            13.min(self.stack.len())
                        } else {
                            need
                        }),
                    )
                    .collect();
                match kind {
                    0 => {
                        self.curve([s[0], 0.0, s[1], s[2], s[3], 0.0]);
                        self.curve([s[4], 0.0, s[5], -s[2], s[6], 0.0]);
                    }
                    1 => {
                        self.curve([s[0], s[1], s[2], s[3], s[4], s[5]]);
                        self.curve([s[6], s[7], s[8], s[9], s[10], s[11]]);
                    }
                    2 => {
                        self.curve([s[0], s[1], s[2], s[3], s[4], 0.0]);
                        self.curve([s[5], 0.0, s[6], s[7], s[8], -(s[1] + s[3] + s[7])]);
                    }
                    _ => {
                        let dx = s[0] + s[2] + s[4] + s[6] + s[8];
                        let dy = s[1] + s[3] + s[5] + s[7] + s[9];
                        self.curve([s[0], s[1], s[2], s[3], s[4], s[5]]);
                        if dx.abs() > dy.abs() {
                            self.curve([s[6], s[7], s[8], s[9], s[10], -dy]);
                        } else {
                            self.curve([s[6], s[7], s[8], s[9], -dx, s[10]]);
                        }
                    }
                }
                self.stack.clear();
            }
            Op::Unknown => self.stack.clear(),
        }
        Ok(false)
    }
}

/// Reads the number starting at `pos`; returns (value, next position).
fn number(data: &[u8], pos: usize) -> R<(f64, usize)> {
    let b = data[pos];
    let at = |i: usize, what: &str| {
        data.get(i)
            .copied()
            .ok_or_else(|| format!("Unexpected end of CharString{what}"))
    };
    Ok(match b {
        28 => {
            let v = (u16::from(at(pos + 1, " reading shortint")?) << 8)
                | u16::from(at(pos + 2, " reading shortint")?);
            (f64::from(v as i16), pos + 3)
        }
        32..=246 => (f64::from(b) - 139.0, pos + 1),
        247..=250 => (
            f64::from((u16::from(b) - 247) * 256 + u16::from(at(pos + 1, "")?) + 108),
            pos + 2,
        ),
        251..=254 => (
            -f64::from((u16::from(b) - 251) * 256 + u16::from(at(pos + 1, "")?) + 108),
            pos + 2,
        ),
        _ => {
            let s = data
                .get(pos + 1..pos + 5)
                .ok_or("Unexpected end of CharString reading fixed-point")?;
            (
                f64::from(i32::from_be_bytes([s[0], s[1], s[2], s[3]])) / 65536.0,
                pos + 5,
            )
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // 10 20 hstemhm | hintmask <mask> | 5 rlineto
    const BODY: [u8; 2] = [144, 5]; // `5 rlineto` operand + opcode follow below

    fn prog(mask: u8) -> Vec<u8> {
        let mut v = vec![149, 159, 18, 19, mask];
        v.push(BODY[0]);
        v.push(BODY[1]);
        v
    }

    #[test]
    fn main_hintmask_does_not_skip_mask_bytes() {
        // Ruby's `@io` is nil at top level: the mask byte (number 10) stays
        // on the stack and becomes the dx of the rlineto.
        let mut it = Interp::new(&[], None);
        it.run_main(&prog(149)).unwrap();
        assert_eq!(it.path, vec![Seg::Line(10.0, 5.0)]);
    }

    #[test]
    fn subroutine_hintmask_skips_mask_bytes() {
        // The same bytes in a local subroutine: one mask byte is skipped.
        let sub = prog(149);
        let subs: [&[u8]; 1] = [&sub];
        // `-107 callsubr` (bias 107 for fewer than 1240 subrs).
        let mut it = Interp::new(&[], Some(&subs));
        it.run_main(&[32, 10]).unwrap();
        assert!(it.path.is_empty(), "rlineto saw only one operand");
    }
}

#[cfg(test)]
mod budget_tests {
    use super::*;

    /// Global subr i calls subr i+1 twice (the #158 break-test font shape):
    /// 2^n operations without an operation budget.
    fn fan_out(n: usize) -> Vec<Vec<u8>> {
        let mut v: Vec<Vec<u8>> = (0..n)
            .map(|i| {
                let arg = u8::try_from(i + 1 + 139 - 107).unwrap();
                vec![arg, 29, arg, 29] // `<i+1> callgsubr` twice
            })
            .collect();
        v.push(vec![11]); // return
        v
    }

    fn run(n: usize) -> (R<()>, std::time::Duration) {
        let subrs = fan_out(n);
        let refs: Vec<&[u8]> = subrs.iter().map(Vec::as_slice).collect();
        let mut it = Interp::new(&refs, None);
        let start = std::time::Instant::now();
        // `-107 callgsubr`: enter subr 0
        let r = it.run_main(&[32, 29]);
        (r, start.elapsed())
    }

    #[test]
    fn exponential_subr_fan_out_fails_fast_with_an_error() {
        let (r, took) = run(60);
        let e = r.unwrap_err();
        assert!(e.contains("budget"), "{e}");
        assert!(took < std::time::Duration::from_secs(2), "{took:?}");
    }

    #[test]
    fn a_legitimate_fan_out_still_runs() {
        // 2^12 calls: well inside the budget.
        let (r, _) = run(12);
        assert!(r.is_ok(), "{r:?}");
    }
}
