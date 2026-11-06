//! The audit report (Ruby `AuditCommand#build_report` / `Models::AuditReport`).
//! Field semantics follow the canonical JSON: absent facts are `None`, and empty
//! lists are omitted when the report is rendered (`axes` is always present).
use super::types::Issue;
use crate::sfnt::{Error, Font, LoadingMode, Options, Tag, detect_format, face_count};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fmt::Write;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Axis {
    pub tag: String,
    pub min_value_raw: i32,
    pub default_value_raw: i32,
    pub max_value_raw: i32,
    pub name_id: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub error_count: usize,
    pub fatal_count: usize,
    pub info_count: usize,
    pub warning_count: usize,
    pub total: usize,
    pub passed: bool,
}

impl Summary {
    pub fn from_issues(issues: &[Issue]) -> Self {
        let n = |s: super::Severity| issues.iter().filter(|i| i.severity == s).count();
        let (error_count, fatal_count) = (n(super::Severity::Error), n(super::Severity::Fatal));
        Summary {
            error_count,
            fatal_count,
            info_count: n(super::Severity::Info),
            warning_count: n(super::Severity::Warning),
            total: issues.len(),
            passed: error_count == 0 && fatal_count == 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub source_file: String,
    pub source_sha256: String,
    pub source_format: String,
    /// `Some` only for collection members.
    pub font_index: Option<usize>,
    pub num_fonts_in_source: usize,
    pub family_name: Option<String>,
    pub subfamily_name: Option<String>,
    pub full_name: Option<String>,
    pub postscript_name: Option<String>,
    pub version: Option<String>,
    /// Present when both `name` and `head` are.
    pub font_revision_raw: Option<i32>,
    pub is_variable: bool,
    pub italic: bool,
    pub bold: bool,
    pub weight_class: Option<u16>,
    pub width_class: Option<u16>,
    pub panose: Option<String>,
    pub axes: Vec<Axis>,
    /// `None` when the font has no `cmap` (Ruby: nil, omitted from the JSON).
    pub total_codepoints: Option<usize>,
    pub cmap_subtables: Vec<u16>,
    pub total_glyphs: usize,
    pub opentype_scripts: Vec<String>,
    pub features: Vec<String>,
    /// `Some` only for a validating audit.
    pub validation_issues: Option<Vec<Issue>>,
    pub validation_summary: Option<Summary>,
}

fn tag_str(t: &Tag) -> String {
    String::from_utf8_lossy(t).into_owned()
}

/// Lowercase hex SHA-256 of the whole source file.
fn file_sha256(bytes: &[u8]) -> String {
    #[cfg(test)]
    FILE_HASHES.with(|c| c.set(c.get() + 1));
    Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

#[cfg(test)]
thread_local! {
    static FILE_HASHES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// `build_report` for one loaded face. `bytes` is the whole source file and
/// `source_sha256` its digest, computed once per file by the caller (hashing a
/// large collection once per face dominated the audit of a 57 MB font).
pub fn build_report(
    face: &Font<'_>,
    bytes: &[u8],
    source_sha256: &str,
    source_file: &str,
    font_index: Option<usize>,
    num_fonts: usize,
    validate: bool,
) -> Result<Report, Error> {
    let mut r = Report {
        source_file: source_file.to_string(),
        source_sha256: source_sha256.to_string(),
        source_format: detect_format(bytes)
            .map_or("unknown", |f| f.as_str())
            .into(),
        font_index,
        num_fonts_in_source: num_fonts,
        family_name: None,
        subfamily_name: None,
        full_name: None,
        postscript_name: None,
        version: None,
        font_revision_raw: None,
        is_variable: false,
        italic: false,
        bold: false,
        weight_class: None,
        width_class: None,
        panose: None,
        axes: vec![],
        total_codepoints: None,
        cmap_subtables: vec![],
        total_glyphs: 0,
        opentype_scripts: vec![],
        features: vec![],
        validation_issues: None,
        validation_summary: None,
    };
    // A table listed in the directory but unreadable is `None` here (Ruby: nil
    // from `face.table`). Its section stays empty, as for an absent table.
    // identity
    if let Some(name) = face.name()? {
        r.family_name = name.english_name(1);
        r.subfamily_name = name.english_name(2);
        r.full_name = name.english_name(4);
        r.postscript_name = name.english_name(6);
        r.version = name.english_name(5);
        if let Some(head) = face.head()? {
            r.font_revision_raw = Some(head.font_revision_raw);
        }
    }
    // style
    if let Some(os2) = face.os2()? {
        r.weight_class = Some(os2.us_weight_class);
        r.width_class = Some(os2.us_width_class);
        r.italic = os2.fs_selection & 0x01 != 0;
        r.bold = os2.fs_selection & 0x20 != 0;
        r.panose = Some(
            os2.panose
                .iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>()
                .join(" "),
        );
    }
    if let Some(head) = face.head()? {
        r.italic |= head.mac_style & 0x02 != 0;
        r.bold |= head.mac_style & 0x01 != 0;
    }
    if let Some(fvar) = face.fvar()? {
        r.is_variable = true;
        r.axes = fvar
            .axes
            .iter()
            .map(|a| Axis {
                tag: a.tag.clone(),
                min_value_raw: a.min_value_raw,
                default_value_raw: a.default_value_raw,
                max_value_raw: a.max_value_raw,
                name_id: a.name_id,
            })
            .collect();
    }
    // coverage
    if let Some(cmap) = face.cmap()? {
        r.total_codepoints = Some(cmap.codepoints().len());
        r.cmap_subtables = cmap.subtable_formats();
    }
    r.total_glyphs = if face.has_table("maxp") {
        face.maxp()
            .ok()
            .flatten()
            .map_or(0, |m| usize::from(m.num_glyphs))
    } else {
        0
    };
    // layout
    let (mut scripts, mut features) = (BTreeSet::new(), BTreeSet::new());
    for tag in ["GSUB", "GPOS"] {
        if !face.has_table(tag) {
            continue;
        }
        let table = if tag == "GSUB" {
            face.gsub()?
        } else {
            face.gpos()?
        };
        if let Some(t) = table {
            // Parsed once and budgeted (as in `read`); `features(script)` per
            // script re-parsed both lists each time and took seconds on a
            // 137 KB table.
            for (script, fs) in t.script_features() {
                scripts.insert(script);
                features.extend(fs);
            }
        }
    }
    r.opentype_scripts = scripts.iter().map(tag_str).collect();
    r.features = features.iter().map(tag_str).collect();
    if validate {
        let issues = super::run_checks(face);
        r.validation_summary = Some(Summary::from_issues(&issues));
        r.validation_issues = Some(issues);
    }
    Ok(r)
}

/// Where an audit failed: Ruby's `load_font` wraps load failures, report building raises as is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Load,
    Report,
}

/// `AuditCommand#run`: one report per face (collections audit every member),
/// each face loaded `full`, eager. WOFF/WOFF2 are decoded by the loader;
/// dfont/Type 1 are `Unsupported`.
pub fn audit(bytes: &[u8], source_file: &str, validate: bool) -> Result<Vec<Report>, Error> {
    audit_staged(bytes, source_file, validate).map_err(|(e, _)| e)
}

/// [`audit`], also saying at which stage a failure happened.
pub fn audit_staged(
    bytes: &[u8],
    source_file: &str,
    validate: bool,
) -> Result<Vec<Report>, (Error, Stage)> {
    let opts = Options {
        mode: LoadingMode::Full,
        lazy: false,
    };
    let is_collection = detect_format(bytes).is_some_and(|f| f.is_collection());
    let sha = std::cell::OnceCell::new();
    let one = |i: usize, index: Option<usize>, num: usize| {
        let face = Font::load(bytes, i, opts).map_err(|e| (e, Stage::Load))?;
        let sha = sha.get_or_init(|| file_sha256(bytes));
        build_report(&face, bytes, sha, source_file, index, num, validate)
            .map_err(|e| (e, Stage::Report))
    };
    if !is_collection {
        return Ok(vec![one(0, None, 1)?]);
    }
    let num = face_count(bytes).unwrap_or(0);
    (0..num).map(|i| one(i, Some(i), num)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A collection of `faces` empty sfnt faces.
    fn collection(faces: u32) -> Vec<u8> {
        let header = 12 + 4 * faces as usize;
        let mut b = b"ttcf\0\x01\0\0".to_vec();
        b.extend_from_slice(&faces.to_be_bytes());
        for i in 0..faces {
            b.extend_from_slice(&((header + 12 * i as usize) as u32).to_be_bytes());
        }
        for _ in 0..faces {
            b.extend_from_slice(&[0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        }
        b
    }

    #[test]
    fn a_collection_audit_hashes_the_file_once_and_every_face_carries_it() {
        let bytes = collection(3);
        FILE_HASHES.with(|c| c.set(0));
        let reports = audit_staged(&bytes, "x.ttc", false).expect("audit");
        assert_eq!(reports.len(), 3);
        assert_eq!(FILE_HASHES.with(|c| c.get()), 1, "file hashed per face");
        let want = file_sha256(&bytes);
        assert!(reports.iter().all(|r| r.source_sha256 == want));
    }

    /// An sfnt whose directory lists `tags`, each with data past the end of the file.
    fn unreadable(tags: &[&str]) -> Vec<u8> {
        let mut b = vec![0, 1, 0, 0];
        b.extend_from_slice(&(tags.len() as u16).to_be_bytes());
        b.extend_from_slice(&[0; 6]);
        for t in tags {
            b.extend_from_slice(t.as_bytes());
            b.extend_from_slice(&[0; 4]);
            b.extend_from_slice(&0x7FFF_FFF0u32.to_be_bytes());
            b.extend_from_slice(&16u32.to_be_bytes());
        }
        b
    }

    #[test]
    fn an_unreadable_table_is_skipped_not_a_nomethoderror() {
        for validate in [false, true] {
            let bytes = unreadable(&["name", "head", "OS/2", "fvar", "cmap"]);
            let r = audit(&bytes, "x.ttf", validate).expect("audit must not fail");
            let r = &r[0];
            assert_eq!(r.family_name, None);
            assert_eq!(r.font_revision_raw, None);
            assert_eq!(r.weight_class, None);
            assert!(!r.is_variable);
            assert!(r.axes.is_empty());
            assert_eq!(r.total_codepoints, None);
        }
    }
}
