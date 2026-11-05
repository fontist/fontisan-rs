//! The audit report (Ruby `AuditCommand#build_report` / `Models::AuditReport`).
//! Field semantics follow the canonical JSON: absent facts are `None`, and empty
//! lists are omitted when the report is rendered (`axes` is always present).
use super::types::Issue;
use crate::sfnt::{Error, ErrorClass, Font, LoadingMode, Options, Tag, detect_format, face_count};
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
    pub total_codepoints: usize,
    pub cmap_subtables: Vec<u16>,
    pub total_glyphs: usize,
    pub opentype_scripts: Vec<String>,
    pub features: Vec<String>,
    /// `Some` only for a validating audit.
    pub validation_issues: Option<Vec<Issue>>,
    pub validation_summary: Option<Summary>,
}

fn nil(method: &str) -> Error {
    Error {
        class: ErrorClass::NoMethod,
        message: format!("undefined method '{method}' for nil"),
        cause: None,
    }
}

fn tag_str(t: &Tag) -> String {
    String::from_utf8_lossy(t).into_owned()
}

/// `build_report` for one loaded face. `bytes` is the whole source file.
pub fn build_report(
    face: &Font<'_>,
    bytes: &[u8],
    source_file: &str,
    font_index: Option<usize>,
    num_fonts: usize,
    validate: bool,
) -> Result<Report, Error> {
    let hex = Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        });
    let mut r = Report {
        source_file: source_file.to_string(),
        source_sha256: hex,
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
        total_codepoints: 0,
        cmap_subtables: vec![],
        total_glyphs: 0,
        opentype_scripts: vec![],
        features: vec![],
        validation_issues: None,
        validation_summary: None,
    };
    // identity
    if face.has_table("name") {
        let name = face.name()?.ok_or_else(|| nil("english_name"))?;
        r.family_name = name.english_name(1);
        r.subfamily_name = name.english_name(2);
        r.full_name = name.english_name(4);
        r.postscript_name = name.english_name(6);
        r.version = name.english_name(5);
        if face.has_table("head") {
            let head = face.head()?.ok_or_else(|| nil("font_revision"))?;
            r.font_revision_raw = Some(head.font_revision_raw);
        }
    }
    // style
    if face.has_table("OS/2") {
        let os2 = face.os2()?.ok_or_else(|| nil("us_weight_class"))?;
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
    if face.has_table("head") {
        let head = face.head()?.ok_or_else(|| nil("mac_style"))?;
        r.italic |= head.mac_style & 0x02 != 0;
        r.bold |= head.mac_style & 0x01 != 0;
    }
    if face.has_table("fvar") {
        r.is_variable = true;
        let fvar = face.fvar()?.ok_or_else(|| nil("axes"))?;
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
    if face.has_table("cmap") {
        let cmap = face.cmap()?.ok_or_else(|| nil("unicode_mappings"))?;
        r.total_codepoints = cmap.codepoints().len();
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
            let s = t.scripts();
            scripts.extend(s.iter().copied());
            for script in &s {
                features.extend(t.features(script));
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

/// `AuditCommand#run`: one report per face (collections audit every member),
/// each face loaded `full`, eager. WOFF/WOFF2/dfont/Type 1 are not read here.
pub fn audit(bytes: &[u8], source_file: &str, validate: bool) -> Result<Vec<Report>, Error> {
    let opts = Options {
        mode: LoadingMode::Full,
        lazy: false,
    };
    let is_collection = detect_format(bytes).is_some_and(|f| f.is_collection());
    if !is_collection {
        let face = Font::load(bytes, 0, opts)?;
        return Ok(vec![build_report(
            &face,
            bytes,
            source_file,
            None,
            1,
            validate,
        )?]);
    }
    let num = face_count(bytes).unwrap_or(0);
    (0..num)
        .map(|i| {
            let face = Font::load(bytes, i, opts)?;
            build_report(&face, bytes, source_file, Some(i), num, validate)
        })
        .collect()
}
