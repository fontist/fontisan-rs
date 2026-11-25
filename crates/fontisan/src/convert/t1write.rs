//! The Type 1 PFB writer (fontisan#192 gap 4, stage 2): the canonical
//! three-segment Adobe layout the Ruby `Type1::PFBWriter` assembles, with
//! Ruby's deterministic encryption - the eexec and CharString ciphers
//! prepend fixed lenIV bytes (`0x00`), so two runs produce identical bytes.

// The converters are groundwork: the parity iteration wires the remaining callers.
#![allow(dead_code)]

use crate::convert::ConvertError;

const EEXEC_KEY: u16 = 55_665;
const CHARSTRING_KEY: u16 = 4_330;
const CIPHER_MULT: u32 = 52_845;
const CIPHER_ADD: u32 = 22_719;
const CIPHER_MASK: u32 = 0xFFFF;
/// The fixed byte repeated for the lenIV prefix (4 times).
const LEN_IV_BYTE: u8 = 0x00;
const PFB_MARKER: u8 = 0x80;
const ASCII_SEGMENT: u8 = 0x01;
const BINARY_SEGMENT: u8 = 0x02;
const END_SEGMENT: u8 = 0x03;
const TRAILER_ZEROS: usize = 512;

/// R4 encryption (symmetric with decryption): each plain byte is XORed with
/// the high byte of the running cipher state, which then advances.
fn encrypt(data: &[u8], key: u16) -> Vec<u8> {
    let mut r = u32::from(key);
    let mut out = Vec::with_capacity(data.len());
    for &plain in data {
        let cipher = (plain ^ ((r >> 8) & 0xFF) as u8) as u32;
        out.push(cipher as u8);
        r = ((cipher + r) * CIPHER_MULT + CIPHER_ADD) & CIPHER_MASK;
    }
    out
}

/// The eexec-encrypted body of a PFB (key 55665, no lenIV).
pub fn eexec_encrypt(data: &[u8]) -> Vec<u8> {
    encrypt(data, EEXEC_KEY)
}

/// The CharString-encrypted form of one glyph's plain bytes (key 4330,
/// four `0x00` lenIV bytes prepended).
pub fn charstring_encrypt(data: &[u8]) -> Vec<u8> {
    if data.is_empty() {
        return Vec::new();
    }
    let mut plain = vec![LEN_IV_BYTE; 4];
    plain.extend_from_slice(data);
    encrypt(&plain, CHARSTRING_KEY)
}

fn segment(marker: u8, data: &[u8]) -> Vec<u8> {
    let mut out = vec![PFB_MARKER, marker];
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
    out
}

/// The named inputs of the canonical PFB, mirroring
/// `Type1::PFBWriter#initialize`. The fontinfo and private values are the
/// already-rendered text (the caller formats parens, floats and arrays the
/// way the Ruby side does).
pub struct Pfb<'a> {
    pub psname: &'a str,
    pub version: &'a str,
    /// FontInfo entries as complete rendered lines ("/Key value def"),
    /// formatted by the caller the way the Ruby side formats per key type.
    pub fontinfo: Vec<String>,
    pub bbox: [i32; 4],
    /// Private dictionary entries in written order: key => rendered value
    /// text (`[a b c]` for arrays, the text itself otherwise).
    pub private_entries: Vec<(&'a str, String)>,
    /// glyph name => plain (unencrypted) Type 1 CharString bytes.
    pub charstrings: Vec<(&'a str, Vec<u8>)>,
}

impl Pfb<'_> {
    /// The clear-text header, ending at the eexec handoff.
    fn clear_text(&self) -> Vec<u8> {
        let fontinfo = self.fontinfo.join("\n");
        let fontinfo = if fontinfo.is_empty() {
            fontinfo
        } else {
            format!("{fontinfo}\n")
        };
        format!(
            "%!PS-AdobeFont-1.0: {} {}\n\
             %%BeginResource: font {}\n\
             12 dict dup begin\n\
             /FontType 1 def\n\
             /FontName /{} def\n\
             /FontInfo {} dict dup begin\n\
             {}end def\n\
             /PaintType 0 def\n\
             /FontMatrix [0.001 0 0 0.001 0 0] def\n\
             /Encoding 256 array\n\
             0 1 255 {{1 index exch /.notdef put}} for\n\
             def\n\
             /FontBBox {{{}}} def\n\
             end\n\
             currentfile eexec\n",
            self.psname,
            self.version,
            self.psname,
            self.psname,
            self.fontinfo.len(),
            fontinfo,
            self.bbox
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(" "),
        )
        .into_bytes()
    }

    /// The eexec-encrypted body: private dictionary, then CharStrings.
    fn encrypted_payload(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(
            b"dup /Private\n\
              8 dict dup begin\n\
              /-| {string currentfile exch readstring pop} def\n\
              /|- {def} def\n\
              /| {put} def\n",
        );
        for (key, value) in &self.private_entries {
            out.extend_from_slice(format!("/{key} {value} def\n").as_bytes());
        }
        out.extend_from_slice(b"end\n/CharStrings\n");
        out.extend_from_slice(format!("{} dict dup begin\n", self.charstrings.len()).as_bytes());
        for (name, plain) in &self.charstrings {
            let encrypted = charstring_encrypt(plain);
            if encrypted.is_empty() {
                continue;
            }
            out.extend_from_slice(format!("/{name} {} -| ", encrypted.len()).as_bytes());
            out.extend_from_slice(&encrypted);
            out.extend_from_slice(b" |-\n");
        }
        out.extend_from_slice(
            b"end put\n\
              end\n\
              dup /FontName get exch definefont pop\n\
              mark\n\
              currentfile closefile\n",
        );
        out
    }

    fn trailer(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for _ in 0..TRAILER_ZEROS / 64 {
            out.extend_from_slice(format!("{}\n", "0".repeat(64)).as_bytes());
        }
        out.extend_from_slice(b"cleartomark\n%%EndResource\n%%EOF\n");
        out
    }

    /// The PFB bytes.
    pub fn write(&self) -> Result<Vec<u8>, ConvertError> {
        let encrypted = eexec_encrypt(&self.encrypted_payload());
        let mut out = Vec::new();
        out.extend_from_slice(&segment(ASCII_SEGMENT, &self.clear_text()));
        out.extend_from_slice(&segment(BINARY_SEGMENT, &encrypted));
        out.extend_from_slice(&segment(ASCII_SEGMENT, &self.trailer()));
        out.extend_from_slice(&segment(END_SEGMENT, b""));
        Ok(out)
    }
}

/// Encode a Type 1 CharString integer (the variable-length encoding).
pub fn encode_number(num: i32) -> Vec<u8> {
    if (-107..=107).contains(&num) {
        vec![(num + 139) as u8]
    } else if (-1131..=1131).contains(&num) {
        if num >= 108 {
            let v = num - 108;
            vec![((v >> 8) + 247) as u8, (v & 0xFF) as u8]
        } else {
            let v = -num - 108;
            vec![((v >> 8) + 251) as u8, (v & 0xFF) as u8]
        }
    } else if (-32768..=32767).contains(&num) {
        vec![255, (num & 0xFF) as u8, ((num >> 8) & 0xFF) as u8]
    } else {
        let mut out = vec![255u8];
        for i in 0..4 {
            out.push(((num >> (8 * i)) & 0xFF) as u8);
        }
        out
    }
}
