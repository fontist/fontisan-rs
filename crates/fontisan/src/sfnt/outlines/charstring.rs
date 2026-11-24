//! Type 2 charstring interpretation (Adobe TN 5177) for CFF faces: outline
//! points through the same [`GlyphOutline`] shape the glyf decoder produces.
//!
//! Charstrings are decrypted with the Type 1 cipher (key 4330, skipping the
//! Private DICT's `lenIV` bytes, default 4), then interpreted against the
//! global and local subr indexes (bias 107/1131/32768, nesting limit 10).
//! Cubic segments become `[c1, c2, on]` off-off-on triples and the contour
//! is flagged [`Contour::cubic`], the convention the Ruby outline extractor
//! uses. Coordinates round to integers like Ruby's `Float#round`. Widths are
//! ignored: advance metrics come from hmtx. Hint arguments feed only the
//! mask-byte arithmetic. `random` refuses: outline geometry must stay
//! deterministic.
use super::cff::CffState;
use super::points::{Contour, GlyphOutline, Point};
use super::{Error, ErrorClass};

const CORRUPTED: &str = "Failed to interpret Type 2 charstring";
const SUBR_LIMIT: usize = 10;
const STACK_LIMIT: usize = 48;
const TRANSIENTS: usize = 32;

fn corrupted(message: impl Into<String>) -> Error {
    Error::new(
        ErrorClass::CorruptedTable,
        format!("{CORRUPTED}: {}", message.into()),
    )
}

/// The Type 1 cipher (eexec/charstring), skipping `len_iv` leading bytes.
fn decrypt(data: &[u8], len_iv: usize) -> Vec<u8> {
    let mut r: u16 = 4330;
    let mut plain = Vec::with_capacity(data.len().saturating_sub(len_iv));
    for (i, &cipher) in data.iter().enumerate() {
        let byte = cipher ^ (r >> 8) as u8;
        r = (u16::from(cipher).wrapping_add(r).wrapping_mul(52845)).wrapping_add(22719);
        if i >= len_iv {
            plain.push(byte);
        }
    }
    plain
}

/// One frame of charstring execution: the program bytes and a read cursor.
struct Frame<'a> {
    bytes: &'a [u8],
    pos: usize,
}

/// The Type 2 interpreter state for one glyph.
struct Interpreter<'a> {
    stack: Vec<f64>,
    transients: [f64; TRANSIENTS],
    stems: usize,
    contours: Vec<Contour>,
    current: Vec<Point>,
    x: f64,
    y: f64,
    /// Whether the next lone line operand continues horizontally.
    horizontal: bool,
    cff: &'a CffState<'a>,
}

impl<'a> Interpreter<'a> {
    /// Decodes one operand per TN 5177 section 3.2; `255` is 16.16 fixed.
    fn number(bytes: &[u8], pos: &mut usize) -> Result<f64, Error> {
        let byte = *bytes
            .get(*pos)
            .ok_or_else(|| corrupted("Unexpected end of charstring"))?;
        *pos += 1;
        let byte8 = |pos: &mut usize| -> Result<f64, Error> {
            let b = *bytes
                .get(*pos)
                .ok_or_else(|| corrupted("Unexpected end of charstring"))?;
            *pos += 1;
            Ok(f64::from(b))
        };
        match byte {
            28 => {
                let hi = byte8(pos)? as u16;
                let lo = byte8(pos)? as u16;
                Ok(f64::from(i16::from_be_bytes([hi as u8, lo as u8])))
            }
            32..=246 => Ok(f64::from(i32::from(byte) - 139)),
            247..=250 => Ok((f64::from(byte) - 247.0) * 256.0 + byte8(pos)? + 108.0),
            251..=254 => Ok(-(f64::from(byte) - 251.0) * 256.0 - byte8(pos)? - 108.0),
            255 => {
                let hi = byte8(pos)? as i16 as i32;
                let mid = byte8(pos)? as i32;
                let lo1 = byte8(pos)? as i32;
                let lo2 = byte8(pos)? as i32;
                let word = hi << 24 | mid << 16 | lo1 << 8 | lo2;
                Ok(f64::from(word) / 65536.0)
            }
            _ => Err(corrupted("Operand byte is an operator")),
        }
    }

    /// Pops `count` operands (widths sit at the stack bottom, so operators
    /// always read from the top per Note 4).
    fn pop<const N: usize>(&mut self) -> Result<[f64; N], Error> {
        if self.stack.len() < N {
            return Err(corrupted("Operator is missing operands"));
        }
        let mut out = [0.0f64; N];
        out.copy_from_slice(&self.stack.split_off(self.stack.len() - N));
        Ok(out)
    }

    fn point(&self, x: f64, y: f64, on_curve: bool) -> Point {
        Point {
            x: x.round() as i32,
            y: y.round() as i32,
            on_curve,
        }
    }

    fn flush(&mut self) {
        if !self.current.is_empty() {
            self.contours.push(Contour {
                points: std::mem::take(&mut self.current),
                cubic: true,
            });
        }
    }

    fn move_to(&mut self, dx: f64, dy: f64) {
        self.flush();
        self.x += dx;
        self.y += dy;
        self.current.push(self.point(self.x, self.y, true));
        self.horizontal = true;
    }

    fn line_to(&mut self, dx: f64, dy: f64) {
        self.x += dx;
        self.y += dy;
        self.current.push(self.point(self.x, self.y, true));
    }

    fn curve_to(&mut self, c1: (f64, f64), c2: (f64, f64), end: (f64, f64)) {
        self.current.push(self.point(c1.0, c1.1, false));
        self.current.push(self.point(c2.0, c2.1, false));
        self.x = end.0;
        self.y = end.1;
        self.current.push(self.point(self.x, self.y, true));
    }

    /// One relative cubic from six deltas (TN 5177 rrcurveto).
    fn rrcurveto(&mut self, [a, b, c, d, e, f]: &[f64; 6]) {
        let (x, y) = (self.x, self.y);
        self.curve_to(
            (x + a, y + b),
            (x + a + c, y + b + d),
            (x + a + c + e, y + b + d + f),
        );
    }

    /// Alternating h/v lines: `horizontal` says what the first operand is.
    fn alternating_lines(&mut self, args: &[f64], mut horizontal: bool) {
        for arg in args {
            if horizontal {
                self.line_to(*arg, 0.0);
            } else {
                self.line_to(0.0, *arg);
            }
            horizontal = !horizontal;
        }
    }

    fn bias(count: usize) -> f64 {
        if count < 1240 {
            107.0
        } else if count < 33900 {
            1131.0
        } else {
            32768.0
        }
    }

    fn stems_flush(&mut self) -> Result<(), Error> {
        self.stems += self.stack.len() / 2;
        if self.stems > 96 {
            return Err(corrupted("Stem hint count exceeds 96"));
        }
        self.stack.clear();
        Ok(())
    }

    /// Interprets until `endchar`, a program end, or `return` at the top
    /// frame, following subroutine calls (TN 5177 section 4.7).
    fn run(&mut self, bytes: &'a [u8]) -> Result<(), Error> {
        let mut frames = vec![Frame { bytes, pos: 0 }];
        while let Some(frame) = frames.last_mut() {
            let Some(&b) = frame.bytes.get(frame.pos) else {
                // Running off a program's end is an implicit `return`; the
                // top frame finishing this way ends the glyph.
                frames.pop();
                if frames.is_empty() {
                    break;
                }
                continue;
            };
            frame.pos += 1;
            match b {
                0 | 2 | 9 | 13 | 15 | 16 | 17 => {
                    return Err(corrupted(format!("Reserved operator byte {b}")));
                }
                12 => {
                    let second = *frame
                        .bytes
                        .get(frame.pos)
                        .ok_or_else(|| corrupted("Two-byte operator ends the charstring"))?;
                    frame.pos += 1;
                    self.arithmetic(second)?;
                }
                1 | 3 | 18 | 23 => self.stems_flush()?,
                19 | 20 => {
                    // hintmask / cntrmask: pending stems, then the mask bytes.
                    self.stems_flush()?;
                    let bytes = self.stems.div_ceil(8);
                    if frame.bytes.len() < frame.pos + bytes {
                        return Err(corrupted("Hint mask runs past the charstring"));
                    }
                    frame.pos += bytes;
                }
                4 => {
                    let [dy] = self.pop()?;
                    self.move_to(0.0, dy);
                }
                21 => {
                    let [dx, dy] = self.pop()?;
                    self.move_to(dx, dy);
                }
                22 => {
                    let [dx] = self.pop()?;
                    self.move_to(dx, 0.0);
                }
                5 => {
                    if self.stack.len() % 2 != 0 {
                        return Err(corrupted("rlineto needs operand pairs"));
                    }
                    let args = std::mem::take(&mut self.stack);
                    for pair in args.chunks(2) {
                        self.line_to(pair[0], pair[1]);
                    }
                }
                6 | 7 => {
                    let args = std::mem::take(&mut self.stack);
                    self.alternating_lines(&args, b == 6);
                }
                8 => {
                    if self.stack.len() % 6 != 0 {
                        return Err(corrupted("rrcurveto needs six-operand groups"));
                    }
                    let args = std::mem::take(&mut self.stack);
                    for group in args.chunks(6) {
                        self.rrcurveto(&[
                            group[0], group[1], group[2], group[3], group[4], group[5],
                        ]);
                    }
                }
                10 | 29 => {
                    let [index] = self.pop()?;
                    let (table, bias) = if b == 10 {
                        (&self.cff.subrs, Self::bias(self.cff.subrs.len()))
                    } else {
                        (&self.cff.gsubrs, Self::bias(self.cff.gsubrs.len()))
                    };
                    // The bias is added to the encoded subr number (TN 5177 2.3).
                    let at = index + bias;
                    if at.fract() != 0.0 || at < 0.0 || at as usize >= table.len() {
                        return Err(corrupted("Subroutine index is out of range"));
                    }
                    if frames.len() >= SUBR_LIMIT {
                        return Err(corrupted("Subroutine nesting exceeds 10"));
                    }
                    let program = table[at as usize];
                    frames.push(Frame {
                        bytes: program,
                        pos: 0,
                    });
                }
                11 => {
                    frames.pop();
                    if frames.is_empty() {
                        break;
                    }
                }
                14 => {
                    // endchar: the optional width and the deprecated seac
                    // arguments carry no outline geometry.
                    self.stack.clear();
                    break;
                }
                24 => {
                    // rcurveline: rrcurveto groups then one final line pair.
                    if self.stack.len() < 8 || (self.stack.len() - 2) % 6 != 0 {
                        return Err(corrupted("rcurveline operand count"));
                    }
                    let args = std::mem::take(&mut self.stack);
                    let (curves, line) = args.split_at(args.len() - 2);
                    for group in curves.chunks(6) {
                        self.rrcurveto(&[
                            group[0], group[1], group[2], group[3], group[4], group[5],
                        ]);
                    }
                    self.line_to(line[0], line[1]);
                }
                25 => {
                    // rlinecurve: line pairs then one final rrcurveto.
                    if self.stack.len() < 8 || (self.stack.len() - 6) % 2 != 0 {
                        return Err(corrupted("rlinecurve operand count"));
                    }
                    let args = std::mem::take(&mut self.stack);
                    let (lines, curve) = args.split_at(args.len() - 6);
                    for pair in lines.chunks(2) {
                        self.line_to(pair[0], pair[1]);
                    }
                    self.rrcurveto(&[curve[0], curve[1], curve[2], curve[3], curve[4], curve[5]]);
                }
                26 => {
                    // vvcurveto: [dx1? {dya dxb dyb dyc}+].
                    let leading = self.stack.len() % 4 == 1;
                    let dx1 = if leading { self.pop::<1>()?[0] } else { 0.0 };
                    let sets = std::mem::take(&mut self.stack);
                    if sets.len() % 4 != 0 {
                        return Err(corrupted("vvcurveto operand count"));
                    }
                    let (x, y) = (self.x, self.y);
                    let mut dy = 0.0;
                    for (index, group) in sets.chunks(4).enumerate() {
                        let [a, b, c, d] = [group[0], group[1], group[2], group[3]];
                        let lead = if index == 0 && leading { dx1 } else { 0.0 };
                        self.curve_to(
                            (x + lead, y + dy + a),
                            (x + lead + b, y + dy + a + c),
                            (x + lead + b, y + dy + a + d),
                        );
                        dy += a + d;
                    }
                }
                27 => {
                    // hhcurveto: [dy1? {dxa dxb dyb dxc}+].
                    let leading = self.stack.len() % 4 == 1;
                    let dy1 = if leading { self.pop::<1>()?[0] } else { 0.0 };
                    let sets = std::mem::take(&mut self.stack);
                    if sets.len() % 4 != 0 {
                        return Err(corrupted("hhcurveto operand count"));
                    }
                    let (x, y) = (self.x, self.y);
                    let mut dx = 0.0;
                    for (index, group) in sets.chunks(4).enumerate() {
                        let [a, b, c, d] = [group[0], group[1], group[2], group[3]];
                        let lead = if index == 0 && leading { dy1 } else { 0.0 };
                        self.curve_to(
                            (x + dx + a, y + lead),
                            (x + dx + a + b, y + lead + c),
                            (x + dx + a + b + d, y + lead + c),
                        );
                        dx += a + d;
                    }
                }
                30 | 31 => {
                    // vhcurveto / hvcurveto: alternating four-operand groups
                    // with an optional trailing fifth on the last group.
                    let first_vertical = b == 30;
                    let trailing = self.stack.len() % 4 == 1;
                    let total = self.stack.len();
                    let args = std::mem::take(&mut self.stack);
                    let groups = (total - usize::from(trailing)) / 4;
                    for index in 0..groups {
                        let at = index * 4;
                        let vertical = first_vertical != (index % 2 == 1);
                        let last = index + 1 == groups;
                        let fifth = if last && trailing { args[at + 4] } else { 0.0 };
                        let group = [args[at], args[at + 1], args[at + 2], args[at + 3]];
                        self.flex_curves(group, vertical, last && trailing, fifth);
                    }
                }
                28 | 32..=254 => {
                    frame.pos -= 1;
                    let value = Self::number(frame.bytes, &mut frame.pos)?;
                    if self.stack.len() >= STACK_LIMIT {
                        return Err(corrupted("Argument stack exceeds 48"));
                    }
                    self.stack.push(value);
                }
                255 => {
                    frame.pos -= 1;
                    let value = Self::number(frame.bytes, &mut frame.pos)?;
                    self.stack.push(value);
                }
            }
        }
        Ok(())
    }

    /// One hv/vh curve group: `vertical` says the START tangent; a trailing
    /// fifth operand shifts the end off its tangent axis (TN 5177 hvcurveto).
    fn flex_curves(
        &mut self,
        [a, b, c, d]: [f64; 4],
        vertical: bool,
        fifth: bool,
        fifth_value: f64,
    ) {
        let (x, y) = (self.x, self.y);
        let (c1, c2, end) = if vertical {
            // Args are (dy1, dx2, dy2, dx3): starts vertical, ends horizontal.
            (
                (x, y + a),
                (x + b, y + a + c),
                if fifth {
                    (x + b + d, y + a + c + fifth_value)
                } else {
                    (x + b + d, y + a + c)
                },
            )
        } else {
            // Args are (dx1, dx2, dy2, dy3): starts horizontal, ends vertical.
            (
                (x + a, y),
                (x + a + b, y + c),
                if fifth {
                    (x + a + b + fifth_value, y + c + d)
                } else {
                    (x + a + b, y + c + d)
                },
            )
        };
        self.curve_to(c1, c2, end);
    }

    /// The Type 2 arithmetic, storage and conditional operators (12 x).
    fn arithmetic(&mut self, second: u8) -> Result<(), Error> {
        let pop = |interpreter: &mut Self| -> Result<f64, Error> {
            interpreter
                .stack
                .pop()
                .ok_or_else(|| corrupted("Operator is missing operands"))
        };
        match second {
            3 => {
                let (b, a) = (pop(self)?, pop(self)?);
                self.stack
                    .push(if a != 0.0 && b != 0.0 { 1.0 } else { 0.0 });
            }
            4 => {
                let (b, a) = (pop(self)?, pop(self)?);
                self.stack
                    .push(if a != 0.0 || b != 0.0 { 1.0 } else { 0.0 });
            }
            5 => {
                let a = pop(self)?;
                self.stack.push(if a == 0.0 { 1.0 } else { 0.0 });
            }
            9 => {
                let a = pop(self)?;
                self.stack.push(a.abs());
            }
            10 => {
                let (b, a) = (pop(self)?, pop(self)?);
                self.stack.push(a + b);
            }
            11 => {
                let (b, a) = (pop(self)?, pop(self)?);
                self.stack.push(a - b);
            }
            12 => {
                let (b, a) = (pop(self)?, pop(self)?);
                self.stack.push(a / b);
            }
            14 => {
                let a = pop(self)?;
                self.stack.push(-a);
            }
            15 => {
                let (b, a) = (pop(self)?, pop(self)?);
                self.stack.push(if a == b { 1.0 } else { 0.0 });
            }
            18 => {
                pop(self)?;
            }
            20 => {
                let value = pop(self)?;
                let index = pop(self)?;
                let at = index as usize;
                if index.fract() != 0.0 || at >= TRANSIENTS {
                    return Err(corrupted("Transient array index is out of range"));
                }
                self.transients[at] = value;
            }
            21 => {
                let index = pop(self)?;
                let at = index as usize;
                if index.fract() != 0.0 || at >= TRANSIENTS {
                    return Err(corrupted("Transient array index is out of range"));
                }
                self.stack.push(self.transients[at]);
            }
            22 => {
                let (v2, v1, s2, s1) = (pop(self)?, pop(self)?, pop(self)?, pop(self)?);
                self.stack.push(if s1 <= v1 { s1 } else { s2 });
                let _ = v2;
            }
            23 => {
                return Err(corrupted("random is refused (outlines stay deterministic)"));
            }
            34 => {
                // hflex: dx1 dx2 dy2 dx3 dx4 dx5 dx6 fd.
                let [dx1, dx2, dy2, dx3, dx4, dx5, dx6, _fd] = self.pop()?;
                let (x, y) = (self.x, self.y);
                self.curve_to(
                    (x + dx1, y),
                    (x + dx1 + dx2, y + dy2),
                    (x + dx1 + dx2 + dx3, y),
                );
                self.curve_to(
                    (self.x + dx4, self.y),
                    (self.x + dx4 + dx5, self.y),
                    (self.x + dx4 + dx5 + dx6, self.y),
                );
            }
            35 => {
                // flex: two full curves; fd is a rendering hint only.
                let [a1, b1, a2, b2, a3, b3, a4, b4, a5, b5, a6, b6, _fd] = self.pop()?;
                self.rrcurveto(&[a1, b1, a2, b2, a3, b3]);
                self.rrcurveto(&[a4, b4, a5, b5, a6, b6]);
            }
            36 => {
                // hflex1: dx1 dy1 dx2 dy2 dx3 dx4 dx5 dy5 dx6; the end
                // returns to the flex's starting y.
                let [dx1, dy1, dx2, dy2, dx3, dx4, dx5, dy5, dx6] = self.pop()?;
                let (x, y) = (self.x, self.y);
                self.curve_to(
                    (x + dx1, y + dy1),
                    (x + dx1 + dx2, y + dy1 + dy2),
                    (x + dx1 + dx2 + dx3, y + dy1 + dy2),
                );
                let join_y = self.y;
                self.curve_to(
                    (self.x + dx4, join_y),
                    (self.x + dx4 + dx5, join_y + dy5),
                    (self.x + dx4 + dx5 + dx6, y),
                );
            }
            37 => {
                // flex1: the end keeps the starting point on the smaller-sum
                // axis and joins+d6 on the larger-sum axis (TN 5177, Fig. 3).
                let [dx1, dy1, dx2, dy2, dx3, dy3, dx4, dy4, dx5, dy5, d6] = self.pop()?;
                let (start_x, start_y) = (self.x, self.y);
                self.rrcurveto(&[dx1, dy1, dx2, dy2, dx3, dy3]);
                let dx = dx1 + dx2 + dx3 + dx4 + dx5;
                let dy = dy1 + dy2 + dy3 + dy4 + dy5;
                let end = if dx.abs() > dy.abs() {
                    (self.x + d6, start_y)
                } else {
                    (start_x, self.y + d6)
                };
                self.curve_to(
                    (self.x + dx4, self.y + dy4),
                    (self.x + dx4 + dx5, self.y + dy4 + dy5),
                    end,
                );
            }
            24 => {
                let (b, a) = (pop(self)?, pop(self)?);
                self.stack.push(a * b);
            }
            26 => {
                let a = pop(self)?;
                self.stack.push(a.sqrt());
            }
            27 => {
                let value = pop(self)?;
                self.stack.push(value);
            }
            28 => {
                let (b, a) = (pop(self)?, pop(self)?);
                self.stack.push(a);
                self.stack.push(b);
            }
            29 => {
                let index = pop(self)?;
                let at = (-index) as usize;
                if index.fract() != 0.0 {
                    return Err(corrupted("index operand must be an integer"));
                }
                let from = if index > 0.0 {
                    self.stack
                        .len()
                        .checked_sub(1 + at)
                        .ok_or_else(|| corrupted("index operand is out of range"))?
                } else {
                    self.stack
                        .len()
                        .checked_sub(1)
                        .ok_or_else(|| corrupted("index on an empty stack"))?
                };
                let value = self.stack[from];
                self.stack.push(value);
            }
            30 => {
                let (count, shift) = (pop(self)?, pop(self)?);
                if count.fract() != 0.0 || shift.fract() != 0.0 || count < 0.0 {
                    return Err(corrupted("roll operands must be non-negative integers"));
                }
                let (count, shift) = (count as usize, shift as i64);
                let at = self
                    .stack
                    .len()
                    .checked_sub(count)
                    .ok_or_else(|| corrupted("roll count is out of range"))?;
                let window = shift.rem_euclid(count as i64) as usize;
                let tail = self.stack.split_off(at);
                let mut rolled = Vec::with_capacity(tail.len());
                rolled.extend_from_slice(&tail[window..]);
                rolled.extend_from_slice(&tail[..window]);
                self.stack.extend(rolled);
            }
            _ => return Err(corrupted(format!("Reserved operator 12 {second}"))),
        }
        Ok(())
    }
}

/// Interprets one glyph's charstring into outline points, or `Ok(None)` for
/// a glyph without contours.
pub(super) fn decode_glyph(
    data: &[u8],
    state: &CffState<'_>,
) -> Result<Option<GlyphOutline>, Error> {
    let program = decrypt(data, state.len_iv);
    let mut interpreter = Interpreter {
        stack: Vec::new(),
        transients: [0.0; TRANSIENTS],
        stems: 0,
        contours: Vec::new(),
        current: Vec::new(),
        x: 0.0,
        y: 0.0,
        horizontal: true,
        cff: state,
    };
    interpreter.run(&program)?;
    interpreter.flush();
    if interpreter.contours.is_empty() {
        return Ok(None);
    }
    Ok(Some(GlyphOutline {
        contours: interpreter.contours,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encrypts fixture bytes the way CFF stores them (`len_iv` dummies).
    fn encrypt(plain: &[u8], len_iv: usize) -> Vec<u8> {
        let mut data: Vec<u8> = (0..len_iv).map(|i| i as u8 ^ 0xa5).collect();
        data.extend_from_slice(plain);
        let mut r: u16 = 4330;
        let mut out = Vec::with_capacity(data.len());
        for &plain_byte in &data {
            let cipher = plain_byte ^ (r >> 8) as u8;
            r = (u16::from(cipher).wrapping_add(r).wrapping_mul(52845)).wrapping_add(22719);
            out.push(cipher);
        }
        out
    }

    fn state() -> CffState<'static> {
        CffState {
            gsubrs: Vec::new(),
            private: None,
            subrs: Vec::new(),
            len_iv: 4,
        }
    }

    fn outline(program: &[u8]) -> Option<GlyphOutline> {
        decode_glyph(&encrypt(program, 4), &state()).unwrap()
    }

    fn points(program: &[u8]) -> Vec<Point> {
        outline(program).unwrap().contours.remove(0).points
    }

    #[test]
    fn rmoveto_and_rlineto_draw_on_curve_points() {
        // 100 100 rmoveto 50 0 rlineto 0 50 rlineto endchar.
        let program = [
            139 + 100,
            139 + 100,
            21,
            139 + 50,
            139,
            5,
            139,
            139 + 50,
            5,
            14,
        ];
        let points = points(&program);
        let expected = [(100, 100), (150, 100), (150, 150)];
        assert_eq!(points.len(), 3);
        for (point, (x, y)) in points.iter().zip(expected) {
            assert_eq!((point.x, point.y), (x, y));
            assert!(point.on_curve);
        }
    }

    #[test]
    fn rrcurveto_builds_an_off_off_on_cubic_triple() {
        // 0 0 rmoveto 10 0 10 10 0 10 rrcurveto endchar.
        let program = [
            139,
            139,
            21,
            139 + 10,
            139,
            139 + 10,
            139 + 10,
            139,
            139 + 10,
            8,
            14,
        ];
        let outline = outline(&program).unwrap();
        let points = &outline.contours[0].points;
        assert_eq!(points.len(), 4);
        assert_eq!((points[0].x, points[0].y), (0, 0));
        assert!(points[0].on_curve);
        assert_eq!((points[1].x, points[1].y), (10, 0));
        assert!(!points[1].on_curve);
        assert_eq!((points[2].x, points[2].y), (20, 10));
        assert!(!points[2].on_curve);
        assert_eq!((points[3].x, points[3].y), (20, 20));
        assert!(points[3].on_curve);
        assert!(outline.contours[0].cubic);
    }

    #[test]
    fn hlineto_alternates_horizontal_then_vertical() {
        // 0 0 rmoveto 10 20 10 hlineto endchar: h 10, v 20, h 10.
        let program = [139, 139, 21, 139 + 10, 139 + 20, 139 + 10, 6, 14];
        let points = points(&program);
        let expected = [(0, 0), (10, 0), (10, 20), (20, 20)];
        assert_eq!(points.len(), 4);
        for (point, (x, y)) in points.iter().zip(expected) {
            assert_eq!((point.x, point.y), (x, y));
        }
    }

    #[test]
    fn the_fixed_number_decodes_sixteen_sixteen() {
        // 255 encodes a 16.16 fixed: 1.5 = 0x00018000; rounding gives 2.
        let program = [139, 139, 21, 255, 0, 1, 128, 0, 139, 5, 14];
        let points = points(&program);
        assert_eq!((points[1].x, points[1].y), (2, 0));
    }

    #[test]
    fn width_operands_are_ignored_and_subrs_expand() {
        // A width operand precedes rmoveto; a local subr holds the line.
        let subr = [139 + 30, 139, 5]; // 30 0 rlineto
        let mut state = state();
        state.subrs.push(&subr);
        // 0 0 rmoveto callsubr endchar: the bias is added to the encoded
        // number, so subr 0 encodes as -107 (28 0xff 0x95) with bias 107.
        let program = [139, 139, 21, 28, 0xff, 0x95, 10, 14];
        let outline = decode_glyph(&encrypt(&program, 4), &state)
            .unwrap()
            .unwrap();
        let points = &outline.contours[0].points;
        assert_eq!((points[1].x, points[1].y), (30, 0));
    }

    #[test]
    fn hintmask_consumes_its_mask_bytes() {
        // Two stems then a hintmask (1 mask byte) then a move.
        let program = [
            139,
            139,
            1, // hstem (1 stem)
            139,
            139,
            3, // vstem (1 stem)
            19,
            0xff, // hintmask + 1 mask byte (2 stems → 1 byte)
            139 + 10,
            139 + 20,
            21, // 10 20 rmoveto
            14,
        ];
        let points = points(&program);
        assert_eq!((points[0].x, points[0].y), (10, 20));
    }

    #[test]
    fn hvcurveto_starts_horizontal_and_ends_vertical() {
        // 0 0 rmoveto 10 20 30 40 hvcurveto endchar: args (dx1 dx2 dy2 dy3):
        // c1 (10,0), c2 (30,30), end (30,70) — the end keeps c2's x.
        let program = [139, 139, 21, 139 + 10, 139 + 20, 139 + 30, 139 + 40, 31, 14];
        let points = points(&program);
        assert_eq!(points.len(), 4);
        assert_eq!((points[1].x, points[1].y), (10, 0));
        assert_eq!((points[2].x, points[2].y), (30, 30));
        assert_eq!((points[3].x, points[3].y), (30, 70));
    }

    #[test]
    fn flex1_supplies_the_missing_end_component() {
        // 0 0 rmoveto flex1 dx sums 20 horizontal vs 4 vertical → the end
        // x comes from d6: two cubics landing at (d6, -dy) from the join.
        let program = [
            139,
            139,
            21, // 0 0 rmoveto
            139 + 10,
            139, // dx1=10 dy1=0
            139 + 5,
            139 + 2, // dx2=5 dy2=2
            139 + 3,
            139, // dx3=3 dy3=0
            139 + 2,
            139, // dx4=2 dy4=0
            139,
            139 + 2, // dx5=0 dy5=2
            139 + 7,
            12,
            37, // d6=7 flex1
            14,
        ];
        let outline = outline(&program).unwrap();
        let pts = &outline.contours[0].points;
        // First cubic ends at the join (18, 2).
        assert_eq!((pts[3].x, pts[3].y), (18, 2));
        // dx sum 20 > dy sum 4: the end x is join + d6 and the end y
        // returns to the flex's starting y (0 here).
        assert_eq!((pts[6].x, pts[6].y), (25, 0));
    }

    #[test]
    fn blank_charstrings_and_reserved_operators_are_typed() {
        assert!(decode_glyph(&encrypt(&[14], 4), &state())
            .unwrap()
            .is_none());
        let error = decode_glyph(&encrypt(&[9], 4), &state()).unwrap_err();
        assert_eq!(error.class, ErrorClass::CorruptedTable);
    }

    #[test]
    fn random_refuses_to_keep_outlines_deterministic() {
        let program = [12, 23, 14];
        let error = decode_glyph(&encrypt(&program, 4), &state()).unwrap_err();
        assert!(error.message.contains("deterministic"));
    }
}
