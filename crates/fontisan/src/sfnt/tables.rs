//! The 1c tables: head, hhea, maxp, OS/2, post, name. Each mirrors the field
//! names and parse rules of the Ruby record (`Fontisan::Tables::*`):
//!  * an empty table body parses to an all-zero record (Ruby: `read("")`);
//!  * fields are read strictly in order, so a short body fails at the first
//!    field that does not fit (see [`Cursor`]);
//!  * version-conditional fields use Ruby's exact conditions (maxp: version
//!    == 0x00010000; OS/2: `version >= n`; post: version == 0x00020000).
use super::cursor::Cursor;
use super::error::Error;
use super::standard_names::STANDARD_NAMES;

/// One snapshot value, shaped like the Ruby facade's `snapshot_value`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Int(i64),
    /// A printable-ASCII string stays text; any other becomes `"0x" + hex`.
    Text(String),
    List(Vec<Value>),
}

/// Ordered `(ruby field name, value)` pairs.
pub type Snapshot = Vec<(&'static str, Value)>;

fn text(bytes: &[u8]) -> Value {
    if bytes.iter().all(|b| (0x20..=0x7e).contains(b)) {
        Value::Text(bytes.iter().map(|&b| b as char).collect())
    } else {
        let mut s = String::from("0x");
        for b in bytes {
            s.push_str(&format!("{b:02x}"));
        }
        Value::Text(s)
    }
}

macro_rules! record {
    ($(#[$m:meta])* $name:ident { $($f:ident : $t:ident),+ $(,)? }, parse($c:ident) $parse:block) => {
        $(#[$m])*
        #[derive(Clone, Debug, Default, PartialEq, Eq)]
        pub struct $name { $(pub $f: $t),+ }
        impl $name {
            /// Ruby field names and values, in declaration order.
            pub fn snapshot(&self) -> Snapshot {
                vec![$((stringify!($f), Value::Int(self.$f as i64))),+]
            }
            pub(crate) fn parse(data: &[u8]) -> Result<Self, Error> {
                if data.is_empty() { return Ok(Self::default()); }
                let mut $c = Cursor::new(data, 0);
                $parse
            }
        }
    };
}

record!(
    /// `head`.
    Head {
        version_raw: i32, font_revision_raw: i32, checksum_adjustment: u32, magic_number: u32,
        flags: u16, units_per_em: u16, created_raw: i64, modified_raw: i64,
        x_min: i16, y_min: i16, x_max: i16, y_max: i16, mac_style: u16,
        lowest_rec_ppem: u16, font_direction_hint: i16, index_to_loc_format: i16,
        glyph_data_format: i16,
    },
    parse(c) {
        Ok(Self {
            version_raw: c.i32()?, font_revision_raw: c.i32()?, checksum_adjustment: c.u32()?,
            magic_number: c.u32()?, flags: c.u16()?, units_per_em: c.u16()?,
            created_raw: c.i64()?, modified_raw: c.i64()?,
            x_min: c.i16()?, y_min: c.i16()?, x_max: c.i16()?, y_max: c.i16()?,
            mac_style: c.u16()?, lowest_rec_ppem: c.u16()?, font_direction_hint: c.i16()?,
            index_to_loc_format: c.i16()?, glyph_data_format: c.i16()?,
        })
    }
);

record!(
    /// `hhea`.
    Hhea {
        version_raw: i32, ascent: i16, descent: i16, line_gap: i16, advance_width_max: u16,
        min_left_side_bearing: i16, min_right_side_bearing: i16, x_max_extent: i16,
        caret_slope_rise: i16, caret_slope_run: i16, caret_offset: i16,
        metric_data_format: i16, number_of_h_metrics: u16,
    },
    parse(c) {
        let version_raw = c.i32()?;
        let (ascent, descent, line_gap) = (c.i16()?, c.i16()?, c.i16()?);
        let advance_width_max = c.u16()?;
        let (min_left_side_bearing, min_right_side_bearing, x_max_extent) = (c.i16()?, c.i16()?, c.i16()?);
        let (caret_slope_rise, caret_slope_run, caret_offset) = (c.i16()?, c.i16()?, c.i16()?);
        c.skip(8)?;
        Ok(Self {
            version_raw, ascent, descent, line_gap, advance_width_max, min_left_side_bearing,
            min_right_side_bearing, x_max_extent, caret_slope_rise, caret_slope_run, caret_offset,
            metric_data_format: c.i16()?, number_of_h_metrics: c.u16()?,
        })
    }
);

/// maxp version 1.0, the only version with the TrueType fields (Ruby: `==`).
const MAXP_VERSION_1_0: i32 = 0x0001_0000;

/// `maxp`. The TrueType fields are `Some` only when `version_raw == 0x00010000`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Maxp {
    pub version_raw: i32,
    pub num_glyphs: u16,
    /// max_points, max_contours, max_composite_points, max_composite_contours,
    /// max_zones, max_twilight_points, max_storage, max_function_defs,
    /// max_instruction_defs, max_stack_elements, max_size_of_instructions,
    /// max_component_elements, max_component_depth.
    pub v1: Option<[u16; 13]>,
}

const MAXP_V1_NAMES: [&str; 13] = [
    "max_points",
    "max_contours",
    "max_composite_points",
    "max_composite_contours",
    "max_zones",
    "max_twilight_points",
    "max_storage",
    "max_function_defs",
    "max_instruction_defs",
    "max_stack_elements",
    "max_size_of_instructions",
    "max_component_elements",
    "max_component_depth",
];

impl Maxp {
    pub fn snapshot(&self) -> Snapshot {
        let mut v = vec![
            ("version_raw", Value::Int(self.version_raw as i64)),
            ("num_glyphs", Value::Int(self.num_glyphs as i64)),
        ];
        if let Some(f) = &self.v1 {
            v.extend(
                MAXP_V1_NAMES
                    .iter()
                    .zip(f)
                    .map(|(n, x)| (*n, Value::Int(*x as i64))),
            );
        }
        v
    }
    pub(crate) fn parse(data: &[u8]) -> Result<Self, Error> {
        if data.is_empty() {
            return Ok(Self::default());
        }
        let mut c = Cursor::new(data, 0);
        let version_raw = c.i32()?;
        let num_glyphs = c.u16()?;
        let v1 = if version_raw == MAXP_VERSION_1_0 {
            let mut f = [0u16; 13];
            for x in &mut f {
                *x = c.u16()?;
            }
            Some(f)
        } else {
            None
        };
        Ok(Self {
            version_raw,
            num_glyphs,
            v1,
        })
    }
}

/// `OS/2`. Optional groups follow Ruby's `version >= n` conditions.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Os2 {
    pub version: u16,
    pub x_avg_char_width: i16,
    pub us_weight_class: u16,
    pub us_width_class: u16,
    pub fs_type: u16,
    /// y_subscript_x_size, _y_size, _x_offset, _y_offset, y_superscript_x_size,
    /// _y_size, _x_offset, _y_offset, y_strikeout_size, y_strikeout_position.
    pub sub_super_strike: [i16; 10],
    pub s_family_class: i16,
    pub panose: [u8; 10],
    pub ul_unicode_range: [u32; 4],
    pub ach_vend_id: [u8; 4],
    pub fs_selection: u16,
    pub us_first_char_index: u16,
    pub us_last_char_index: u16,
    pub s_typo_ascender: i16,
    pub s_typo_descender: i16,
    pub s_typo_line_gap: i16,
    pub us_win_ascent: u16,
    pub us_win_descent: u16,
    /// ul_code_page_range1, 2 (version >= 1).
    pub code_page_range: Option<[u32; 2]>,
    /// sx_height, s_cap_height, us_default_char, us_break_char, us_max_context (version >= 2).
    pub v2: Option<[i32; 5]>,
    /// us_lower_optical_point_size, us_upper_optical_point_size (version >= 5).
    pub optical_point_size: Option<[u16; 2]>,
}

const OS2_SSS_NAMES: [&str; 10] = [
    "y_subscript_x_size",
    "y_subscript_y_size",
    "y_subscript_x_offset",
    "y_subscript_y_offset",
    "y_superscript_x_size",
    "y_superscript_y_size",
    "y_superscript_x_offset",
    "y_superscript_y_offset",
    "y_strikeout_size",
    "y_strikeout_position",
];

impl Os2 {
    pub fn snapshot(&self) -> Snapshot {
        let i = |x: i64| Value::Int(x);
        let mut v: Snapshot = vec![
            ("version", i(self.version as i64)),
            ("x_avg_char_width", i(self.x_avg_char_width as i64)),
            ("us_weight_class", i(self.us_weight_class as i64)),
            ("us_width_class", i(self.us_width_class as i64)),
            ("fs_type", i(self.fs_type as i64)),
        ];
        v.extend(
            OS2_SSS_NAMES
                .iter()
                .zip(self.sub_super_strike)
                .map(|(n, x)| (*n, i(x as i64))),
        );
        v.push(("s_family_class", i(self.s_family_class as i64)));
        v.push((
            "panose",
            Value::List(self.panose.iter().map(|&b| i(b as i64)).collect()),
        ));
        for (n, x) in [
            "ul_unicode_range1",
            "ul_unicode_range2",
            "ul_unicode_range3",
            "ul_unicode_range4",
        ]
        .iter()
        .zip(self.ul_unicode_range)
        {
            v.push((n, i(x as i64)));
        }
        v.push(("ach_vend_id", text(&self.ach_vend_id)));
        v.extend([
            ("fs_selection", i(self.fs_selection as i64)),
            ("us_first_char_index", i(self.us_first_char_index as i64)),
            ("us_last_char_index", i(self.us_last_char_index as i64)),
            ("s_typo_ascender", i(self.s_typo_ascender as i64)),
            ("s_typo_descender", i(self.s_typo_descender as i64)),
            ("s_typo_line_gap", i(self.s_typo_line_gap as i64)),
            ("us_win_ascent", i(self.us_win_ascent as i64)),
            ("us_win_descent", i(self.us_win_descent as i64)),
        ]);
        if let Some([a, b]) = self.code_page_range {
            v.push(("ul_code_page_range1", i(a as i64)));
            v.push(("ul_code_page_range2", i(b as i64)));
        }
        if let Some(f) = self.v2 {
            for (n, x) in [
                "sx_height",
                "s_cap_height",
                "us_default_char",
                "us_break_char",
                "us_max_context",
            ]
            .iter()
            .zip(f)
            {
                v.push((n, i(x as i64)));
            }
        }
        if let Some([a, b]) = self.optical_point_size {
            v.push(("us_lower_optical_point_size", i(a as i64)));
            v.push(("us_upper_optical_point_size", i(b as i64)));
        }
        v
    }

    pub(crate) fn parse(data: &[u8]) -> Result<Self, Error> {
        if data.is_empty() {
            return Ok(Self {
                ach_vend_id: [0; 4],
                ..Self::default()
            });
        }
        let mut c = Cursor::new(data, 0);
        let mut o = Self {
            version: c.u16()?,
            x_avg_char_width: c.i16()?,
            ..Self::default()
        };
        o.us_weight_class = c.u16()?;
        o.us_width_class = c.u16()?;
        o.fs_type = c.u16()?;
        for x in &mut o.sub_super_strike {
            *x = c.i16()?;
        }
        o.s_family_class = c.i16()?;
        for x in &mut o.panose {
            *x = c.u8()?;
        }
        for x in &mut o.ul_unicode_range {
            *x = c.u32()?;
        }
        o.ach_vend_id.copy_from_slice(c.bytes(4)?);
        o.fs_selection = c.u16()?;
        o.us_first_char_index = c.u16()?;
        o.us_last_char_index = c.u16()?;
        o.s_typo_ascender = c.i16()?;
        o.s_typo_descender = c.i16()?;
        o.s_typo_line_gap = c.i16()?;
        o.us_win_ascent = c.u16()?;
        o.us_win_descent = c.u16()?;
        if o.version >= 1 {
            o.code_page_range = Some([c.u32()?, c.u32()?]);
        }
        if o.version >= 2 {
            o.v2 = Some([
                c.i16()? as i32,
                c.i16()? as i32,
                c.u16()? as i32,
                c.u16()? as i32,
                c.u16()? as i32,
            ]);
        }
        if o.version >= 5 {
            o.optical_point_size = Some([c.u16()?, c.u16()?]);
        }
        Ok(o)
    }
}

/// `post`: the 32-byte header, `num_glyphs_v2` for version 2.0, and the rest.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Post {
    pub version_raw: i32,
    pub italic_angle_raw: i32,
    pub underline_position: i16,
    pub underline_thickness: i16,
    pub is_fixed_pitch: u32,
    pub min_mem_type42: u32,
    pub max_mem_type42: u32,
    pub min_mem_type1: u32,
    pub max_mem_type1: u32,
    pub num_glyphs_v2: Option<u16>,
    pub remaining_data: Vec<u8>,
}

const POST_VERSION_2_0: i32 = 0x0002_0000;
const POST_VERSION_1_0: i32 = 0x0001_0000;

impl Post {
    pub(crate) fn parse(data: &[u8]) -> Result<Self, Error> {
        if data.is_empty() {
            return Ok(Self::default());
        }
        let mut c = Cursor::new(data, 0);
        let mut p = Self {
            version_raw: c.i32()?,
            italic_angle_raw: c.i32()?,
            underline_position: c.i16()?,
            underline_thickness: c.i16()?,
            is_fixed_pitch: c.u32()?,
            min_mem_type42: c.u32()?,
            max_mem_type42: c.u32()?,
            min_mem_type1: c.u32()?,
            max_mem_type1: c.u32()?,
            ..Self::default()
        };
        if p.version_raw == POST_VERSION_2_0 {
            p.num_glyphs_v2 = Some(c.u16()?);
        }
        p.remaining_data = c.rest().to_vec();
        Ok(p)
    }

    /// Glyph names as Ruby's `Post#glyph_names`: standard names for 1.0, the
    /// format-2 index/Pascal-string list for 2.0, empty otherwise.
    pub fn glyph_names(&self) -> Vec<String> {
        self.glyph_names_raw()
            .into_iter()
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .collect()
    }

    /// The same names as raw bytes: Ruby's custom names are binary strings, so a
    /// name that is not UTF-8 must reach the digest unchanged.
    pub fn glyph_names_raw(&self) -> Vec<Vec<u8>> {
        if self.version_raw == POST_VERSION_1_0 {
            return STANDARD_NAMES.iter().map(|s| s.as_bytes().to_vec()).collect();
        }
        if self.version_raw != POST_VERSION_2_0 || self.remaining_data.is_empty() {
            return Vec::new();
        }
        let data = &self.remaining_data;
        let mut offset = 0;
        let mut indices = Vec::new();
        for _ in 0..self.num_glyphs_v2.unwrap_or(0) {
            if offset + 2 > data.len() {
                break;
            }
            indices.push(u16::from_be_bytes([data[offset], data[offset + 1]]) as usize);
            offset += 2;
        }
        let mut custom: Vec<&[u8]> = Vec::new();
        while offset < data.len() {
            let len = data[offset] as usize;
            offset += 1;
            if len == 0 || offset + len > data.len() {
                break;
            }
            custom.push(&data[offset..offset + len]);
            offset += len;
        }
        indices
            .into_iter()
            .map(|i| {
                if i < 258 {
                    STANDARD_NAMES[i].as_bytes().to_vec()
                } else {
                    custom
                        .get(i - 258)
                        .map(|b| b.to_vec())
                        .unwrap_or_else(|| b".notdef".to_vec())
                }
            })
            .collect()
    }
}

/// One `name` record (6 x uint16).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NameRecord {
    pub platform_id: u16,
    pub encoding_id: u16,
    pub language_id: u16,
    pub name_id: u16,
    pub string_length: u16,
    pub string_offset: u16,
}

/// `name`. `storage` is everything after the record array (Ruby's `rest`),
/// NOT the data at the header's `string_offset`: Ruby ignores that field.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Name {
    pub format: u16,
    pub record_count: u16,
    pub string_offset: u16,
    pub records: Vec<NameRecord>,
    pub storage: Vec<u8>,
}

impl Name {
    pub(crate) fn parse(data: &[u8]) -> Result<Self, Error> {
        if data.is_empty() {
            return Ok(Self::default());
        }
        let mut c = Cursor::new(data, 0);
        let (format, record_count, string_offset) = (c.u16()?, c.u16()?, c.u16()?);
        let mut records = Vec::with_capacity((record_count as usize).min(data.len() / 12));
        for _ in 0..record_count {
            records.push(NameRecord {
                platform_id: c.u16()?,
                encoding_id: c.u16()?,
                language_id: c.u16()?,
                name_id: c.u16()?,
                string_length: c.u16()?,
                string_offset: c.u16()?,
            });
        }
        Ok(Self {
            format,
            record_count,
            string_offset,
            records,
            storage: c.rest().to_vec(),
        })
    }

    /// The facade's `decode_name` (fontist's `decode_name_record_manually`):
    /// `None` for an empty or out-of-range string. Invalid bytes become U+FFFD.
    pub fn decode(&self, r: &NameRecord) -> Option<String> {
        let (off, len) = (r.string_offset as usize, r.string_length as usize);
        if len == 0 || off + len > self.storage.len() {
            return None;
        }
        let d = &self.storage[off..off + len];
        Some(match r.platform_id {
            0 | 3 => decode_utf16be(d),
            1 => d
                .iter()
                .map(|&b| if b < 0x80 { b as char } else { '\u{fffd}' })
                .collect(),
            _ => String::from_utf8_lossy(d).into_owned(),
        })
    }

    /// Ruby's `Name#english_name`: Windows/en-US, else Macintosh/English.
    /// Same decode rules and the same out-of-range => `None`.
    pub fn english_name(&self, name_id: u16) -> Option<String> {
        let find = |platform, language| {
            self.records.iter().find(|r| {
                r.name_id == name_id && r.platform_id == platform && r.language_id == language
            })
        };
        self.decode(find(3, 0x0409).or_else(|| find(1, 0))?)
    }
}

fn decode_utf16be(d: &[u8]) -> String {
    let units = d.chunks_exact(2).map(|p| u16::from_be_bytes([p[0], p[1]]));
    let mut s: String = char::decode_utf16(units)
        .map(|r| r.unwrap_or('\u{fffd}'))
        .collect();
    if d.len() % 2 == 1 {
        s.push('\u{fffd}');
    }
    s
}

#[cfg(test)]
mod post_name_tests {
    use super::*;

    /// flip_post: a custom glyph name that is not UTF-8 stays raw bytes (Ruby reads it
    /// as a binary string), so the digest over the joined names matches Ruby's.
    #[test]
    fn version_2_custom_name_keeps_non_utf8_bytes() {
        let mut raw = vec![0, 2, 0, 0];
        raw.extend_from_slice(&[0; 28]);
        raw.extend_from_slice(&[0, 1, 1, 2]); // one glyph, index 258
        raw.extend_from_slice(&[2, 0xFF, 0x80]);
        let p = Post::parse(&raw).unwrap();
        assert_eq!(p.glyph_names_raw(), vec![vec![0xFF, 0x80]]);
    }
}
