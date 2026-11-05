use super::cmap::Cmap;
use super::cursor::Cursor;
use super::error::{Error, ErrorClass};
use super::tables::{Head, Hhea, Maxp, Name, Os2, Post};
use std::collections::HashMap;

const UNKNOWN_FORMAT: &str =
    "Unknown font format. Expected TTF, OTF, TTC, OTC, WOFF, WOFF2, PFB, or PFA file.";

/// Tables the `metadata` loading mode makes available (Ruby `LoadingModes`).
pub const METADATA_TABLES: [&str; 6] = ["name", "head", "hhea", "maxp", "OS/2", "post"];

/// Ruby `FONTISAN_MODE`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LoadingMode {
    Metadata,
    #[default]
    Full,
}

impl LoadingMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Metadata => "metadata",
            Self::Full => "full",
        }
    }
    fn allows(self, tag: &str) -> bool {
        self == Self::Full || METADATA_TABLES.contains(&tag)
    }
}

/// Ruby `FontLoader.load(path, mode:, lazy:)`. Defaults match Ruby's: full, eager.
///
/// `lazy` changes WHEN Ruby reads table bytes, not what a caller sees, except
/// in two degenerate cases reproduced here: duplicate tags (lazy reads the
/// first entry, eager the last) and the metadata batch read, which only
/// happens eagerly. Collections are always loaded eagerly, as in Ruby.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Options {
    pub mode: LoadingMode,
    pub lazy: bool,
}

/// On-disk format by magic bytes (`FontLoader.detect_format`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Ttf,
    Otf,
    Ttc,
    Otc,
    Woff,
    Woff2,
    Woff2Collection,
    Dfont,
    Pfa,
    Pfb,
}

impl Format {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ttf => "ttf",
            Self::Otf => "otf",
            Self::Ttc => "ttc",
            Self::Otc => "otc",
            Self::Woff => "woff",
            Self::Woff2 => "woff2",
            Self::Woff2Collection => "woff2_collection",
            Self::Dfont => "dfont",
            Self::Pfa => "pfa",
            Self::Pfb => "pfb",
        }
    }
    /// Ruby class a loaded font of this format has (`Fontisan::TrueTypeFont`...).
    pub fn ruby_class(self) -> &'static str {
        match self {
            Self::Ttf | Self::Ttc => "Fontisan::TrueTypeFont",
            Self::Otf | Self::Otc => "Fontisan::OpenTypeFont",
            Self::Woff => "Fontisan::WoffFont",
            Self::Woff2 | Self::Woff2Collection => "Fontisan::Woff2Font",
            Self::Dfont => "Fontisan::TrueTypeFont",
            Self::Pfa | Self::Pfb => "Fontisan::Type1Font",
        }
    }
    fn font_type(self) -> &'static str {
        match self {
            Self::Otf | Self::Otc => "OpenTypeFont",
            _ => "TrueTypeFont",
        }
    }
    pub fn is_collection(self) -> bool {
        matches!(
            self,
            Self::Ttc | Self::Otc | Self::Woff2Collection | Self::Dfont
        )
    }
}

fn sfnt_kind(sig: &[u8]) -> Option<Format> {
    match sig {
        [0, 1, 0, 0] | b"true" => Some(Format::Ttf),
        b"OTTO" => Some(Format::Otf),
        _ => None,
    }
}

/// Offsets of a `ttcf` header's fonts and its flavour (`otc` if any member is
/// CFF); `None` where Ruby's `scan_collection` returns nil (truncated header,
/// offset past EOF, unrecognised member magic).
fn scan_collection(b: &[u8]) -> Option<(Vec<u32>, Format)> {
    let mut c = Cursor::new(b, 4);
    c.u16().ok()?;
    c.u16().ok()?;
    let n = c.u32().ok()?;
    let mut offs = Vec::new();
    for _ in 0..n {
        offs.push(c.u32().ok()?);
    }
    let mut otf = false;
    for &o in &offs {
        let sig = b.get(o as usize..(o as usize).checked_add(4)?)?;
        if sfnt_kind(sig)? == Format::Otf {
            otf = true;
        }
    }
    Some((offs, if otf { Format::Otc } else { Format::Ttc }))
}

/// Content-based format detection, as Ruby's `FontLoader.detect`.
pub fn detect_format(b: &[u8]) -> Option<Format> {
    let head = b.get(..4);
    let found = match head {
        Some(b"ttcf") => scan_collection(b).map(|(_, f)| f),
        Some(b"wOFF") => Some(Format::Woff),
        Some(b"wOF2") => Some(if b.get(4..8) == Some(b"ttcf") {
            Format::Woff2Collection
        } else {
            Format::Woff2
        }),
        Some([0, 0, 1, 0]) => Some(Format::Dfont),
        Some(sig) => sfnt_kind(sig),
        None => None,
    };
    if found.is_some() {
        return found;
    }
    if b.len() >= 2 && matches!(u16::from_be_bytes([b[0], b[1]]), 0x8001 | 0x8002) {
        return Some(Format::Pfb);
    }
    // Ruby probes only the first 100 bytes for the Type 1 header.
    let probe = &b[..b.len().min(100)];
    let stripped = probe
        .iter()
        .position(|c| !c.is_ascii_whitespace())
        .map_or(&probe[probe.len()..], |i| &probe[i..]);
    (stripped.starts_with(b"%!PS-AdobeFont-1.0")
        || stripped.starts_with(b"%!PS-Adobe-3.0 Resource-Font"))
    .then_some(Format::Pfa)
}

/// Number of faces in a file: the member count of a collection, else 1.
/// `None` when the format is not recognised.
pub fn face_count(b: &[u8]) -> Option<usize> {
    match detect_format(b)? {
        Format::Ttc | Format::Otc => scan_collection(b).map(|(o, _)| o.len()),
        _ => Some(1),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TableEntry {
    pub tag: [u8; 4],
    pub checksum: u32,
    pub offset: u32,
    pub length: u32,
}

impl TableEntry {
    pub fn tag_str(&self) -> String {
        String::from_utf8_lossy(&self.tag).into_owned()
    }
}

/// One entry of [`Font::table_summaries`]: the facade's table digest row minus
/// the hash (hash `Font::table_data` yourself).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableSummary {
    pub tag: String,
    /// Bytes Ruby holds for the table; `None` when absent past EOF or gated out by the mode.
    pub length: Option<usize>,
    /// Parse failure class. Only the 1c tables are parsed here; every other table is `None`.
    pub error: Option<ErrorClass>,
}

type Range = Option<(usize, usize)>;

/// A parsed sfnt face over borrowed bytes (a single font or one member of a
/// collection). `bytes` may be a WOFF decoder's output (card 1e).
pub struct Font<'a> {
    bytes: &'a [u8],
    format: Format,
    options: Options,
    pub sfnt_version: u32,
    entries: Vec<TableEntry>,
    /// Eagerly read table bytes by tag (last entry wins). Empty when lazy.
    eager: HashMap<[u8; 4], Range>,
}

impl<'a> Font<'a> {
    /// Face 0, `full` mode, eager: the entry point for already-decoded sfnt bytes.
    pub fn from_bytes(bytes: &'a [u8]) -> Result<Self, Error> {
        Self::load(bytes, 0, Options::default())
    }

    /// `FontLoader.load(path, font_index:, mode:, lazy:)` over in-memory bytes.
    pub fn load(bytes: &'a [u8], font_index: usize, options: Options) -> Result<Self, Error> {
        let format = detect_format(bytes)
            .ok_or_else(|| Error::new(ErrorClass::InvalidFont, UNKNOWN_FORMAT))?;
        match format {
            Format::Ttf | Format::Otf => {
                let mut f = Self::read_directory(bytes, 0, format, options)
                    .map_err(|e| e.wrap_load(format.font_type()))?;
                f.read_table_data(options.lazy)?;
                Ok(f)
            }
            Format::Ttc | Format::Otc => {
                let (offs, _) = scan_collection(bytes).expect("detected as a collection");
                if font_index >= offs.len() {
                    return Err(Error::new(
                        ErrorClass::InvalidFont,
                        format!(
                            "Font index {font_index} out of range (collection has {} fonts)",
                            offs.len()
                        ),
                    ));
                }
                // Ruby's collection path neither wraps errors nor honours `lazy`.
                let mut f =
                    Self::read_directory(bytes, offs[font_index] as usize, format, options)?;
                f.read_table_data(false)?;
                Ok(f)
            }
            other => Err(Error::new(
                ErrorClass::Unsupported,
                format!("{} is not read by the native sfnt engine", other.as_str()),
            )),
        }
    }

    fn read_directory(
        bytes: &'a [u8],
        at: usize,
        format: Format,
        options: Options,
    ) -> Result<Self, Error> {
        let mut c = Cursor::new(bytes, at);
        let sfnt_version = c.u32()?;
        let num_tables = c.u16()?;
        c.u16()?; // search_range
        c.u16()?; // entry_selector
        c.u16()?; // range_shift
        let mut entries = Vec::with_capacity((num_tables as usize).min(bytes.len() / 16));
        for _ in 0..num_tables {
            let tag: [u8; 4] = c.bytes(4)?.try_into().unwrap();
            entries.push(TableEntry {
                tag,
                checksum: c.u32()?,
                offset: c.u32()?,
                length: c.u32()?,
            });
        }
        Ok(Self {
            bytes,
            format,
            options,
            sfnt_version,
            entries,
            eager: HashMap::new(),
        })
    }

    /// Ruby's `io.seek(off); io.read(len)`.
    fn read_at(&self, off: usize, len: usize) -> Range {
        if len == 0 {
            Some((off.min(self.bytes.len()), 0))
        } else if off >= self.bytes.len() {
            None
        } else {
            Some((off, len.min(self.bytes.len() - off)))
        }
    }

    fn read_table_data(&mut self, lazy: bool) -> Result<(), Error> {
        if lazy {
            return Ok(());
        }
        if self.options.mode == LoadingMode::Full {
            for e in &self.entries {
                let r = self.read_at(e.offset as usize, e.length as usize);
                self.eager.insert(e.tag, r);
            }
            return Ok(());
        }
        // metadata: Ruby's page-aware batched read (8 KiB gap threshold).
        let mut ents: Vec<TableEntry> = self
            .entries
            .iter()
            .copied()
            .filter(|e| METADATA_TABLES.contains(&e.tag_str().as_str()))
            .collect();
        ents.sort_by_key(|e| e.offset);
        let mut i = 0;
        while i < ents.len() {
            let start = ents[i];
            let mut end = start;
            let mut batch = vec![start];
            let mut j = i + 1;
            while j < ents.len() {
                let gap = ents[j].offset as i64 - (end.offset as i64 + end.length as i64);
                if gap > 8192 {
                    break;
                }
                end = ents[j];
                batch.push(ents[j]);
                j += 1;
            }
            if batch.len() == 1 {
                let r = self.read_at(start.offset as usize, start.length as usize);
                self.eager.insert(start.tag, r);
            } else {
                let blen = (end.offset as usize + end.length as usize)
                    .saturating_sub(start.offset as usize);
                let Some((bo, bl)) = self.read_at(start.offset as usize, blen) else {
                    return Err(Error::new(
                        ErrorClass::NoMethod,
                        "undefined method '[]' for nil",
                    ));
                };
                for e in &batch {
                    let rel = (e.offset - start.offset) as usize;
                    let r = if rel > bl {
                        None
                    } else {
                        Some((bo + rel, (e.length as usize).min(bl - rel)))
                    };
                    self.eager.insert(e.tag, r);
                }
            }
            i = j;
        }
        Ok(())
    }

    pub fn format(&self) -> Format {
        self.format
    }
    pub fn mode(&self) -> LoadingMode {
        self.options.mode
    }
    pub fn lazy(&self) -> bool {
        self.options.lazy
    }
    pub fn entries(&self) -> &[TableEntry] {
        &self.entries
    }
    /// Tags in directory order (`SfntFont#table_names`).
    pub fn table_names(&self) -> Vec<String> {
        self.entries.iter().map(TableEntry::tag_str).collect()
    }
    pub fn has_table(&self, tag: &str) -> bool {
        self.entries.iter().any(|e| e.tag_str() == tag)
    }

    /// The table bytes Ruby holds after `font.table(tag)`: short when the table
    /// runs past EOF, `None` when it starts past EOF or the mode gates it out.
    pub fn table_data(&self, tag: &str) -> Option<&'a [u8]> {
        if !self.options.mode.allows(tag) {
            return None;
        }
        let range = if self.options.lazy && self.format != Format::Ttc && self.format != Format::Otc
        {
            let e = self.entries.iter().find(|e| e.tag_str() == tag)?;
            self.read_at(e.offset as usize, e.length as usize)
        } else {
            let e = self.entries.iter().find(|e| e.tag_str() == tag)?;
            *self.eager.get(&e.tag)?
        };
        range.map(|(o, l)| &self.bytes[o..o + l])
    }

    /// Ruby's mode check in `SfntFont#table`: `Err(ArgumentError)` for a
    /// present table the mode gates out, `Ok(None)` for an absent one.
    fn available(&self, tag: &str) -> Result<Option<Option<&'a [u8]>>, Error> {
        if !self.has_table(tag) {
            return Ok(None);
        }
        if !self.options.mode.allows(tag) {
            return Err(Error::new(
                ErrorClass::Argument,
                format!(
                    "Table '{tag}' is not available in {} mode. Available tables: {:?}",
                    self.options.mode.as_str(),
                    METADATA_TABLES
                ),
            ));
        }
        Ok(Some(self.table_data(tag)))
    }

    fn parsed<T>(
        &self,
        tag: &str,
        parse: fn(&[u8]) -> Result<T, Error>,
    ) -> Result<Option<T>, Error> {
        match self.available(tag)? {
            Some(Some(d)) => parse(d).map(Some),
            _ => Ok(None),
        }
    }

    pub fn head(&self) -> Result<Option<Head>, Error> {
        self.parsed("head", Head::parse)
    }
    pub fn hhea(&self) -> Result<Option<Hhea>, Error> {
        self.parsed("hhea", Hhea::parse)
    }
    pub fn maxp(&self) -> Result<Option<Maxp>, Error> {
        self.parsed("maxp", Maxp::parse)
    }
    pub fn os2(&self) -> Result<Option<Os2>, Error> {
        self.parsed("OS/2", Os2::parse)
    }
    pub fn post(&self) -> Result<Option<Post>, Error> {
        self.parsed("post", Post::parse)
    }
    pub fn name(&self) -> Result<Option<Name>, Error> {
        self.parsed("name", Name::parse)
    }
    pub fn cmap(&self) -> Result<Option<Cmap>, Error> {
        self.parsed("cmap", Cmap::parse)
    }

    /// The facade's per-table digest rows (everything but the sha256).
    pub fn table_summaries(&self) -> Vec<TableSummary> {
        self.entries
            .iter()
            .map(|e| {
                let tag = e.tag_str();
                let (length, error) = match self.available(&tag) {
                    Err(_) => (None, None), // gated by the loading mode
                    Ok(_) => {
                        let err = match tag.as_str() {
                            "head" => self.head().err(),
                            "hhea" => self.hhea().err(),
                            "maxp" => self.maxp().err(),
                            "OS/2" => self.os2().err(),
                            "post" => self.post().err(),
                            "name" => self.name().err(),
                            "cmap" => self.cmap().err(),
                            _ => None,
                        };
                        match err {
                            Some(e) => (None, Some(e.class)),
                            None => (self.table_data(&tag).map(<[u8]>::len), None),
                        }
                    }
                };
                TableSummary { tag, length, error }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Header + directory for `(tag, offset, length)` entries, then `tail` bytes.
    fn font(entries: &[(&[u8; 4], u32, u32)], tail: &[u8]) -> Vec<u8> {
        let mut v = vec![0, 1, 0, 0];
        v.extend((entries.len() as u16).to_be_bytes());
        v.extend([0; 6]);
        for (t, o, l) in entries {
            v.extend(*t);
            v.extend(0u32.to_be_bytes());
            v.extend(o.to_be_bytes());
            v.extend(l.to_be_bytes());
        }
        v.extend(tail);
        v
    }

    #[test]
    fn directory_overrun_is_ruby_runtime_error_with_eof_cause() {
        let mut b = font(&[(b"head", 0, 0)], &[]);
        b[4..6].copy_from_slice(&100u16.to_be_bytes()); // numTables far past EOF
        let e = Font::from_bytes(&b).err().unwrap();
        assert_eq!(e.class, ErrorClass::Runtime);
        assert_eq!(e.message, "Invalid TrueTypeFont file: End of file reached");
        assert_eq!(
            e.cause,
            Some((ErrorClass::Eof, "End of file reached".into()))
        );
    }

    #[test]
    fn partial_field_is_unwrapped_io_error() {
        let e = Font::from_bytes(&[0, 1, 0, 0, 0]).err().unwrap();
        assert_eq!(
            (e.class, e.message.as_str()),
            (ErrorClass::Io, "data truncated")
        );
    }

    #[test]
    fn unknown_magic_is_invalid_font_error() {
        for b in [&b""[..], b"abc", b"garbage!"] {
            assert_eq!(
                Font::from_bytes(b).err().unwrap().class,
                ErrorClass::InvalidFont
            );
        }
    }

    #[test]
    fn table_past_eof_is_short_or_absent_and_empty_table_parses_to_zeros() {
        // head: asks 54 bytes at offset 60 of a 66-byte file, so 6 are available;
        // maxp: starts past EOF; hhea: zero length.
        let b = font(
            &[(b"head", 60, 54), (b"hhea", 99, 0), (b"maxp", 99, 6)],
            &[1, 2, 3, 4, 5, 6],
        );
        let f = Font::from_bytes(&b).unwrap();
        assert_eq!(f.table_data("head").unwrap().len(), 6);
        assert_eq!(f.table_data("maxp"), None);
        assert_eq!(f.table_data("hhea"), Some(&[][..]));
        assert_eq!(f.head().unwrap_err().class, ErrorClass::Io); // 6 bytes: version ok, revision partial
        assert_eq!(f.maxp().unwrap(), None);
        assert_eq!(f.hhea().unwrap().unwrap(), Hhea::default());
    }

    #[test]
    fn metadata_batch_over_a_nil_buffer_is_ruby_no_method_error() {
        // two metadata tables within the gap threshold, both starting past EOF
        let b = font(&[(b"head", 500, 54), (b"hhea", 600, 36)], &[]);
        let opts = Options {
            mode: LoadingMode::Metadata,
            lazy: false,
        };
        assert_eq!(
            Font::load(&b, 0, opts).err().unwrap().class,
            ErrorClass::NoMethod
        );
        // lazy skips the batched read, as Ruby does
        assert!(Font::load(&b, 0, Options { lazy: true, ..opts }).is_ok());
    }

    #[test]
    fn maxp_only_reads_truetype_fields_for_exactly_version_1_0() {
        let mut d = vec![0, 1, 0, 1, 0, 5]; // version 0x00010001: not 1.0
        let m = Maxp::parse(&d).unwrap();
        assert_eq!((m.num_glyphs, m.v1), (5, None));
        d[0..4].copy_from_slice(&0x0001_0000i32.to_be_bytes());
        assert_eq!(Maxp::parse(&d).unwrap_err().class, ErrorClass::Eof);
    }
}
