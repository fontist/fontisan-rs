//! Port of `Tables::Name` as far as the TrueType to CFF conversion needs it:
//! `english_name` and the post-write validator's name checks.

pub(super) struct Name {
    pub format: u16,
    recs: Vec<Rec>,
    storage: Vec<u8>,
}

struct Rec {
    platform: u16,
    enc: u16,
    lang: u16,
    id: u16,
    len: usize,
    off: usize,
}

fn u16_at(d: &[u8], p: usize) -> u16 {
    u16::from_be_bytes([d[p], d[p + 1]])
}

impl Name {
    /// Tolerant parse: a short or empty table has no records.
    pub fn parse(d: &[u8]) -> Self {
        if d.len() < 6 {
            return Self {
                format: 0,
                recs: vec![],
                storage: vec![],
            };
        }
        let count = usize::from(u16_at(d, 2));
        let mut recs = Vec::new();
        for i in 0..count {
            let p = 6 + i * 12;
            if p + 12 > d.len() {
                break;
            }
            recs.push(Rec {
                platform: u16_at(d, p),
                enc: u16_at(d, p + 2),
                lang: u16_at(d, p + 4),
                id: u16_at(d, p + 6),
                len: usize::from(u16_at(d, p + 8)),
                off: usize::from(u16_at(d, p + 10)),
            });
        }
        // Ruby: string_storage is the `rest` after the record array.
        let start = (6 + count * 12).min(d.len());
        Self {
            format: u16_at(d, 0),
            recs,
            storage: d[start..].to_vec(),
        }
    }

    /// `Name#english_name`: Windows/0x409, else Mac/0; UTF-8 bytes.
    pub fn english(&self, id: u16) -> Option<Vec<u8>> {
        let find = |p: u16, l: u16| {
            self.recs
                .iter()
                .find(|r| r.id == id && r.platform == p && r.lang == l)
        };
        let r = find(3, 0x409).or_else(|| find(1, 0))?;
        if r.off + r.len > self.storage.len() || r.len == 0 {
            return None;
        }
        let raw = &self.storage[r.off..r.off + r.len];
        Some(if r.platform == 3 {
            utf16be_lossy(raw).into_bytes()
        } else {
            // ASCII-8BIT -> UTF-8 with undef: :replace
            raw.iter()
                .map(|&b| {
                    if b < 0x80 {
                        (b as char).to_string()
                    } else {
                        "\u{FFFD}".to_string()
                    }
                })
                .collect::<String>()
                .into_bytes()
        })
    }

    pub fn has_combo(&self, p: u16, e: u16, l: u16) -> bool {
        self.recs
            .iter()
            .any(|r| r.platform == p && r.enc == e && r.lang == l)
    }
}

fn utf16be_lossy(raw: &[u8]) -> String {
    let units: Vec<u16> = raw
        .chunks_exact(2)
        .map(|c| u16::from_be_bytes([c[0], c[1]]))
        .collect();
    let mut s: String = char::decode_utf16(units)
        .map(|r| r.unwrap_or('\u{FFFD}'))
        .collect();
    if raw.len() % 2 == 1 {
        s.push('\u{FFFD}');
    }
    s
}
