//! Port of `Variation::Instancing::NameRewriter`.

use super::bytes::*;

struct Record {
    platform: u16,
    encoding: u16,
    language: u16,
    name_id: u16,
    text: String,
}

pub(crate) struct NameRewriter {
    records: Vec<Record>,
}

fn decode(platform: u16, raw: &[u8]) -> String {
    if platform == 0 || platform == 3 {
        let mut units: Vec<u16> = raw
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        let mut s = String::from_utf16_lossy(&units);
        if raw.len() % 2 == 1 {
            s.push('\u{FFFD}');
        }
        units.clear();
        s
    } else {
        raw.iter().map(|&b| b as char).collect()
    }
}

fn encode(r: &Record) -> Vec<u8> {
    if r.platform == 0 || r.platform == 3 {
        r.text.encode_utf16().flat_map(u16::to_be_bytes).collect()
    } else {
        r.text
            .chars()
            .map(|c| {
                if (c as u32) < 256 {
                    c as u32 as u8
                } else {
                    b'?'
                }
            })
            .collect()
    }
}

fn strip(s: &str) -> &str {
    s.trim_matches(|c| matches!(c, '\0' | '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' '))
}

impl NameRewriter {
    pub fn new(data: &[u8]) -> R<Self> {
        let count = usize::from(u16_(data, 2)?);
        let str_off = usize::from(u16_(data, 4)?);
        let mut records = Vec::new();
        for i in 0..count {
            let p = 6 + i * 12;
            let (plat, enc, lang, nid, len, off) = (
                u16_(data, p)?,
                u16_(data, p + 2)?,
                u16_(data, p + 4)?,
                u16_(data, p + 6)?,
                usize::from(u16_(data, p + 8)?),
                usize::from(u16_(data, p + 10)?),
            );
            let raw = sl(data, str_off + off, len)?;
            if lang < 0x8000 {
                records.push(Record {
                    platform: plat,
                    encoding: enc,
                    language: lang,
                    name_id: nid,
                    text: decode(plat, raw),
                });
            }
        }
        Ok(Self { records })
    }

    pub fn lookup(&self, name_id: u16) -> Option<&str> {
        let mut same = self.records.iter().filter(|r| r.name_id == name_id);
        let all: Vec<&Record> = same.by_ref().collect();
        all.iter()
            .find(|r| r.platform == 3 && r.language == 0x409)
            .or_else(|| all.iter().find(|r| r.platform == 3))
            .or_else(|| all.first())
            .map(|r| r.text.as_str())
    }

    fn set(&mut self, name_id: u16, text: &str) {
        let mut any_win = false;
        for r in self.records.iter_mut().filter(|r| r.name_id == name_id) {
            r.text = text.to_string();
            any_win |= r.platform == 3;
        }
        if !any_win {
            self.records.push(Record {
                platform: 3,
                encoding: 1,
                language: 0x409,
                name_id,
                text: text.to_string(),
            });
        }
    }

    pub fn rename(&mut self, label: &str) {
        let family = self
            .lookup(16)
            .or_else(|| self.lookup(1))
            .unwrap_or("Font")
            .to_string();
        let legacy = ["Regular", "Bold", "Italic", "Bold Italic"].contains(&label);
        let full = strip(&format!("{family} {label}")).to_string();
        let ps = format!("{family}-{label}").replace(' ', "");
        let v5 = self.lookup(5).unwrap_or("").to_string();
        self.set(1, if legacy { &family } else { &full });
        self.set(2, if legacy { label } else { "Regular" });
        let n3 = format!("{v5};{ps}");
        self.set(3, n3.strip_prefix(';').unwrap_or(&n3));
        self.set(4, &full);
        self.set(6, &ps);
        self.set(16, &family);
        self.set(17, label);
        self.records.retain(|r| r.name_id != 25);
    }

    pub fn to_binary(&self) -> Vec<u8> {
        let mut recs: Vec<&Record> = self.records.iter().collect();
        recs.sort_by_key(|r| (r.platform, r.encoding, r.language, r.name_id));
        let mut strings = Vec::new();
        let mut entries = Vec::new();
        for r in &recs {
            let bin = encode(r);
            entries.push((r, bin.len(), strings.len()));
            strings.extend_from_slice(&bin);
        }
        let mut out = Vec::new();
        out.extend_from_slice(&0u16.to_be_bytes());
        out.extend_from_slice(&(recs.len() as u16).to_be_bytes());
        out.extend_from_slice(&((6 + recs.len() * 12) as u16).to_be_bytes());
        for (r, len, off) in entries {
            for v in [
                r.platform, r.encoding, r.language, r.name_id, len as u16, off as u16,
            ] {
                out.extend_from_slice(&v.to_be_bytes());
            }
        }
        out.extend_from_slice(&strings);
        out
    }
}
