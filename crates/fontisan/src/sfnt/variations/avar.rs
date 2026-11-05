use crate::sfnt::{Error, cursor::Cursor};

/// avar version 1 segment maps; F2Dot14 values stay raw `i16`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Avar {
    pub version: (u16, u16),
    /// One `(from, to)` list per axis, in axis order.
    pub segment_maps: Vec<Vec<(i16, i16)>>,
}

impl Avar {
    pub(crate) fn parse(data: &[u8]) -> Result<Self, Error> {
        let mut c = Cursor::new(data, 0);
        let version = (c.u16()?, c.u16()?);
        c.u16()?; // reserved
        let axis_count = c.u16()?;
        let mut segment_maps = Vec::new();
        for _ in 0..axis_count {
            let n = c.u16()?;
            let mut map = Vec::new();
            for _ in 0..n {
                map.push((c.i16()?, c.i16()?));
            }
            segment_maps.push(map);
        }
        Ok(Self {
            version,
            segment_maps,
        })
    }
}
