//! Port of `Variation::Instancing::ItemVariationStore` (HVAR advance deltas).

use super::bytes::*;

pub(crate) struct Hvar<'a> {
    hvar: &'a [u8],
    store: &'a [u8],
    advance_map_offset: usize,
    /// Per region, per axis: (start, peak, end).
    regions: Vec<Vec<(f64, f64, f64)>>,
    data_offsets: Vec<usize>,
}

impl<'a> Hvar<'a> {
    pub fn new(hvar: &'a [u8]) -> R<Self> {
        let store_offset = u32_(hvar, 4)? as usize;
        let advance_map_offset = u32_(hvar, 8)? as usize;
        let store = hvar.get(store_offset..).ok_or_else(bad)?;
        let region_list_offset = u32_(store, 2)? as usize;
        let data_count = usize::from(u16_(store, 6)?);
        let axis_count = usize::from(u16_(store, region_list_offset)?);
        let region_count = usize::from(u16_(store, region_list_offset + 2)?);
        let regions = (0..region_count)
            .map(|r| {
                let pos = region_list_offset + 4 + r * axis_count * 6;
                (0..axis_count)
                    .map(|a| {
                        let p = pos + a * 6;
                        let f = |q| i16_(store, q).map(|v| f64::from(v) / 16384.0);
                        Ok((f(p)?, f(p + 2)?, f(p + 4)?))
                    })
                    .collect::<R<Vec<_>>>()
            })
            .collect::<R<Vec<_>>>()?;
        let data_offsets = (0..data_count)
            .map(|i| u32_(store, 8 + i * 4).map(|v| v as usize))
            .collect::<R<Vec<_>>>()?;
        Ok(Self {
            hvar,
            store,
            advance_map_offset,
            regions,
            data_offsets,
        })
    }

    pub fn advance_delta(&self, gid: usize, coords: &[f64]) -> R<f64> {
        let (outer, inner) = self.delta_index(gid)?;
        self.delta(outer, inner, coords)
    }

    fn delta_index(&self, gid: usize) -> R<(usize, usize)> {
        if self.advance_map_offset == 0 {
            return Ok((0, gid));
        }
        let o = self.advance_map_offset;
        let format = u8_(self.hvar, o)?;
        let entry_format = u8_(self.hvar, o + 1)?;
        let (count, pos) = if format == 0 {
            (usize::from(u16_(self.hvar, o + 2)?), o + 4)
        } else {
            (u32_(self.hvar, o + 2)? as usize, o + 6)
        };
        let inner_bits = u32::from(entry_format & 0x0F) + 1;
        let size = usize::from((entry_format & 0x30) >> 4) + 1;
        let last = count.checked_sub(1).ok_or_else(bad)?;
        let entry_gid = gid.min(last);
        let entry = sl(self.hvar, pos + entry_gid * size, size)?
            .iter()
            .fold(0usize, |a, &b| (a << 8) | usize::from(b));
        Ok((entry >> inner_bits, entry & ((1usize << inner_bits) - 1)))
    }

    fn delta(&self, outer: usize, inner: usize, coords: &[f64]) -> R<f64> {
        let Some(&offset) = self.data_offsets.get(outer) else {
            return Ok(0.0);
        };
        let s = self.store;
        let item_count = usize::from(u16_(s, offset)?);
        let word_field = u16_(s, offset + 2)?;
        let region_count = usize::from(u16_(s, offset + 4)?);
        if inner >= item_count {
            return Ok(0.0);
        }
        let long = word_field & 0x8000 != 0;
        let word_count = usize::from(word_field & 0x7FFF);
        let big = if long { 4 } else { 2 };
        let small = if long { 2 } else { 1 };
        let row = word_count * big + region_count.saturating_sub(word_count) * small;
        let pos = offset + 6 + region_count * 2 + inner * row;
        let mut terms = Vec::with_capacity(region_count);
        for i in 0..region_count {
            let index = usize::from(u16_(s, offset + 6 + i * 2)?);
            let d: i64 = if i < word_count {
                if long {
                    i64::from(i32_(s, pos + i * 4)?)
                } else {
                    i64::from(i16_(s, pos + i * 2)?)
                }
            } else {
                let at = pos + word_count * big + (i - word_count) * small;
                if long {
                    i64::from(i16_(s, at)?)
                } else {
                    i64::from(u8_(s, at)? as i8)
                }
            };
            let region = self.regions.get(index).ok_or_else(bad)?;
            terms.push(d as f64 * region_scalar(region, coords));
        }
        Ok(ruby_sum(&terms))
    }
}

fn region_scalar(region: &[(f64, f64, f64)], coords: &[f64]) -> f64 {
    let mut scalar = 1.0;
    for (i, &(lower, peak, upper)) in region.iter().enumerate() {
        let v = coords.get(i).copied().unwrap_or(0.0);
        if peak == 0.0 || lower > peak || peak > upper {
            continue;
        }
        if lower < 0.0 && upper > 0.0 {
            continue;
        }
        if v == peak {
            continue;
        }
        if v <= lower || upper <= v {
            return 0.0;
        }
        scalar *= if v < peak {
            (v - lower) / (peak - lower)
        } else {
            (upper - v) / (upper - peak)
        };
    }
    scalar
}

/// Ruby `Enumerable#sum` over floats (Kahan-Babuska); `0.0` for no terms
/// (Ruby returns Integer 0, which rounds identically).
fn ruby_sum(xs: &[f64]) -> f64 {
    if xs.is_empty() {
        return 0.0;
    }
    let (mut f, mut c) = (0.0f64, 0.0f64);
    for &x in xs {
        if f.is_nan() {
            continue;
        }
        if x.is_nan() {
            f = x;
            continue;
        }
        if x.is_infinite() {
            f = if f.is_infinite() && x.is_sign_negative() != f.is_sign_negative() {
                f64::NAN
            } else {
                x
            };
            continue;
        }
        if f.is_infinite() {
            continue;
        }
        let t = f + x;
        if f.abs() >= x.abs() {
            c += (f - t) + x;
        } else {
            c += (x - t) + f;
        }
        f = t;
    }
    f + c
}
