//! Port of `Variation::Instancing::AxisSpace`.

use super::bytes::*;
use crate::convert::ConvertError;

pub(crate) struct Axis {
    pub tag: [u8; 4],
    pub min: f64,
    pub default: f64,
    pub max: f64,
}

pub(crate) struct AxisSpace {
    pub axes: Vec<Axis>,
    segments: Vec<Vec<(f64, f64)>>,
}

fn fixed(d: &[u8], pos: usize) -> R<f64> {
    Ok(f64::from(i32_(d, pos)?) / 65536.0)
}

fn header(fvar: &[u8]) -> R<(usize, usize, usize, usize, usize)> {
    let g = |p| u16_(fvar, p).map(usize::from);
    Ok((g(4)?, g(8)?, g(10)?, g(12)?, g(14)?))
}

/// `AxisSpace.named_instances`: `(name_id, values in fvar axis order)`.
pub(crate) fn named_instances(fvar: &[u8]) -> R<Vec<(u16, Vec<f64>)>> {
    let (offset, axis_count, axis_size, inst_count, inst_size) = header(fvar)?;
    let base = offset + axis_count * axis_size;
    (0..inst_count)
        .map(|i| {
            let pos = base + i * inst_size;
            let vals = (0..axis_count)
                .map(|a| fixed(fvar, pos + 4 + a * 4))
                .collect::<R<Vec<_>>>()?;
            Ok((u16_(fvar, pos)?, vals))
        })
        .collect()
}

impl AxisSpace {
    pub fn new(fvar: &[u8], avar: Option<&[u8]>) -> R<Self> {
        let (offset, axis_count, axis_size, _, _) = header(fvar)?;
        let axes = (0..axis_count)
            .map(|i| {
                let pos = offset + i * axis_size;
                let t = sl(fvar, pos, 4)?;
                Ok(Axis {
                    tag: [t[0], t[1], t[2], t[3]],
                    min: fixed(fvar, pos + 4)?,
                    default: fixed(fvar, pos + 8)?,
                    max: fixed(fvar, pos + 12)?,
                })
            })
            .collect::<R<Vec<_>>>()?;
        let mut segments = vec![Vec::new(); axes.len()];
        if let Some(avar) = avar {
            let n = usize::from(u16_(avar, 6)?);
            let mut pos = 8;
            for i in 0..n {
                let count = usize::from(u16_(avar, pos)?);
                pos += 2;
                let pairs = (0..count)
                    .map(|j| {
                        Ok((
                            f64::from(i16_(avar, pos + j * 4)?) / 16384.0,
                            f64::from(i16_(avar, pos + j * 4 + 2)?) / 16384.0,
                        ))
                    })
                    .collect::<R<Vec<_>>>()?;
                pos += count * 4;
                if let Some(slot) = segments.get_mut(i) {
                    *slot = pairs;
                }
            }
        }
        Ok(Self { axes, segments })
    }

    /// `validate`: full user location in axis order, defaults filled.
    pub fn validate(&self, requested: &[([u8; 4], f64)]) -> Result<Vec<f64>, ConvertError> {
        let known: Vec<String> = self.axes.iter().map(|a| tag_str(&a.tag)).collect();
        for (tag, _) in requested {
            if !self.axes.iter().any(|a| &a.tag == tag) {
                return Err(ConvertError::new(
                    "Fontisan::InvalidAxisError",
                    format!(
                        "Invalid variation axis '{}': font has axes {}",
                        tag_str(tag),
                        known.join(", ")
                    ),
                ));
            }
        }
        self.axes
            .iter()
            .map(|a| {
                let v = requested
                    .iter()
                    .rev()
                    .find(|(t, _)| *t == a.tag)
                    .map_or(a.default, |(_, v)| *v);
                if v >= a.min && v <= a.max {
                    Ok(v)
                } else {
                    Err(ConvertError::new(
                        "Fontisan::InvalidCoordinatesError",
                        format!(
                            "Invalid coordinate for axis '{}': {}",
                            tag_str(&a.tag),
                            rb_float(v)
                        ),
                    ))
                }
            })
            .collect()
    }

    /// `normalize`: avar-mapped coordinates in -1..1, fvar order.
    pub fn normalize(&self, location: &[f64]) -> Vec<f64> {
        self.axes
            .iter()
            .zip(location)
            .enumerate()
            .map(|(i, (a, &v))| {
                let n = if v < a.default {
                    (v - a.default) / (a.default - a.min)
                } else if v > a.default {
                    (v - a.default) / (a.max - a.default)
                } else {
                    0.0
                };
                map_segment(&self.segments[i], n.clamp(-1.0, 1.0))
            })
            .collect()
    }
}

fn map_segment(pairs: &[(f64, f64)], v: f64) -> f64 {
    if pairs.is_empty() {
        return v;
    }
    if let Some(&(_, to)) = pairs.iter().find(|(from, _)| *from == v) {
        return to;
    }
    match pairs.iter().position(|(from, _)| *from > v) {
        None | Some(0) => v,
        Some(i) => {
            let (f0, t0) = pairs[i - 1];
            let (f1, t1) = pairs[i];
            t0 + ((t1 - t0) * (v - f0) / (f1 - f0))
        }
    }
}

pub(crate) fn tag_str(tag: &[u8; 4]) -> String {
    String::from_utf8_lossy(tag).into_owned()
}
