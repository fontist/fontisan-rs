use crate::sfnt::{Error, cursor::Cursor};

/// One fvar axis; Fixed 16.16 values are the raw integers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FvarAxis {
    pub tag: String,
    pub min_value_raw: i32,
    pub default_value_raw: i32,
    pub max_value_raw: i32,
    pub flags: u16,
    pub name_id: u16,
}

/// One named instance. `postscript_name_id` is present only when the instance
/// record is large enough for it, like Ruby's `Tables::Fvar#instances`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FvarInstance {
    pub name_id: u16,
    pub flags: u16,
    pub postscript_name_id: Option<u16>,
    pub coordinates_raw: Vec<i32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fvar {
    pub axes: Vec<FvarAxis>,
    pub instances: Vec<FvarInstance>,
}

impl Fvar {
    /// Mirrors Ruby: header, then `axis_count` records at `axes_array_offset`
    /// (`axis_size` apart), then instances right after the axes; an instance that
    /// does not fit in the table is dropped (Ruby `compact`s it).
    pub(crate) fn parse(data: &[u8]) -> Result<Self, Error> {
        let mut c = Cursor::new(data, 0);
        c.u16()?; // major
        c.u16()?; // minor
        let axes_offset = usize::from(c.u16()?);
        c.u16()?; // reserved
        let axis_count = usize::from(c.u16()?);
        let axis_size = usize::from(c.u16()?);
        let instance_count = usize::from(c.u16()?);
        let instance_size = usize::from(c.u16()?);

        let mut axes = Vec::with_capacity(axis_count);
        for i in 0..axis_count {
            let at = axes_offset + i * axis_size;
            let rec = data.get(at..).map(|r| &r[..axis_size.min(r.len())]);
            let mut a = Cursor::new(rec.unwrap_or(&[]), 0);
            axes.push(FvarAxis {
                tag: a.bytes(4)?.iter().map(|&b| char::from(b)).collect(),
                min_value_raw: a.i32()?,
                default_value_raw: a.i32()?,
                max_value_raw: a.i32()?,
                flags: a.u16()?,
                name_id: a.u16()?,
            });
        }

        let first = axes_offset + axis_count * axis_size;
        let mut instances = Vec::new();
        for i in 0..instance_count {
            let at = first + i * instance_size;
            if at + instance_size > data.len() || instance_size == 0 {
                continue;
            }
            let mut r = Cursor::new(&data[at..at + instance_size], 0);
            let name_id = r.u16()?;
            let flags = r.u16()?;
            let coordinates_raw = (0..axis_count)
                .map(|_| r.i32())
                .collect::<Result<Vec<_>, _>>()?;
            let postscript_name_id = if instance_size >= 4 + axis_count * 4 + 2 {
                Some(r.u16()?)
            } else {
                None
            };
            instances.push(FvarInstance {
                name_id,
                flags,
                postscript_name_id,
                coordinates_raw,
            });
        }
        Ok(Self { axes, instances })
    }
}
