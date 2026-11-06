//! `scan`: one row per font face with exactly the fields of Ruby's
//! `Fontisan::Scan` / `Reference.scan` (docs/reference-facade.md, "Scan row"),
//! built from the sfnt model and the `indexability` profile.
//!
//! dfont and Type 1 (PFA/PFB) are not read natively: such a file yields ONE row
//! with [`RowStatus::Unsupported`] and no other fields, for the caller to route
//! to Ruby. Everything else, including WOFF/WOFF2, collections and malformed
//! input, is reproduced, failures included: what fontist would raise is recorded
//! in the row's [`RowErrors`], never raised.
use crate::indexability::{self, IndexabilityReport};
pub use crate::json::Json;
use crate::sfnt::{Error, Font, Format, LoadingMode, Name, Options, detect_format, face_count};
use sha2::{Digest, Sha256};

const FONT_FILE_ERROR: &str = "Fontist::Errors::FontFileError";
const INDEXABILITY_ERROR: &str = "Fontist::Errors::FontIndexabilityValidationError";
const NOT_A_COLLECTION_TEXT: &str = "File is not a recognized font collection (TTC/OTC/dfont)";
const RIBBI: [&str; 4] = ["regular", "bold", "italic", "bold italic"];

/// `(key, name id)` of the six name fields, in row order.
const NAME_FIELDS: [(&str, u16); 6] = [
    ("family_name", 1),
    ("full_name", 4),
    ("subfamily_name", 2),
    ("preferred_family", 16),
    ("preferred_subfamily", 17),
    ("postscript_name", 6),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowStatus {
    Ok,
    /// dfont / Type 1: not read natively; route the file to Ruby.
    Unsupported,
}

/// `{class, message}`: Ruby's `error_info`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErrInfo {
    pub class: String,
    pub message: String,
}

/// `{class, message, cause}` with fontist's wrapping.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowError {
    pub class: String,
    pub message: String,
    pub cause: Option<ErrInfo>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RowErrors {
    pub font_file: Option<RowError>,
    pub collection_file: Option<RowError>,
    pub collection_member: Option<ErrInfo>,
}

/// The six name fields, in [`NAME_FIELDS`] order; empty when the font has no
/// `name` table (Ruby `{}`).
pub type NameMap = Vec<(&'static str, Option<String>)>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instance {
    pub family: String,
    pub style: String,
    pub full_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub status: RowStatus,
    pub path: String,
    pub collection_index: Option<usize>,
    pub sha256: Option<String>,
    pub format: Option<String>,
    pub is_collection: bool,
    pub num_faces: usize,
    pub has_fvar: bool,
    pub names: Option<NameMap>,
    pub english_names: Option<NameMap>,
    pub variable_instances: Vec<Instance>,
    pub validation: Option<IndexabilityReport>,
    pub errors: RowErrors,
}

/// Scan a file on disk. A read failure becomes a row, like any other failure.
pub fn scan_path(path: &std::path::Path) -> Vec<Row> {
    let name = path.to_string_lossy().into_owned();
    match std::fs::read(path) {
        Ok(bytes) => scan_bytes(&bytes, &name),
        Err(e) => vec![unreadable_row(&name, &e)],
    }
}

/// Scan font bytes; `path` is the row's `path` as given.
pub fn scan_bytes(bytes: &[u8], path: &str) -> Vec<Row> {
    let format = detect_format(bytes);
    let mut facts = Facts {
        path: path.to_string(),
        sha256: Some(hex(&Sha256::digest(bytes))),
        format: format.map(|f| f.as_str().to_string()),
        is_collection: format.is_some_and(Format::is_collection),
    };
    if matches!(format, Some(Format::Dfont | Format::Pfa | Format::Pfb)) {
        let mut row = facts.base(None, 0);
        row.status = RowStatus::Unsupported;
        return vec![row];
    }
    if !facts.is_collection {
        return vec![face_row(bytes, &facts, None, 1)];
    }
    let count = face_count(bytes).unwrap_or(0);
    if count == 0 {
        return vec![empty_collection_row(bytes, &mut facts)];
    }
    (0..count)
        .map(|i| face_row(bytes, &facts, Some(i), count))
        .collect()
}

struct Facts {
    path: String,
    sha256: Option<String>,
    format: Option<String>,
    is_collection: bool,
}

impl Facts {
    fn base(&self, index: Option<usize>, num_faces: usize) -> Row {
        Row {
            status: RowStatus::Ok,
            path: self.path.clone(),
            collection_index: index,
            sha256: self.sha256.clone(),
            format: self.format.clone(),
            is_collection: self.is_collection,
            num_faces,
            has_fvar: false,
            names: None,
            english_names: None,
            variable_instances: Vec::new(),
            validation: None,
            errors: RowErrors::default(),
        }
    }
}

fn hex(d: &[u8]) -> String {
    use std::fmt::Write;
    d.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn unreadable_row(path: &str, e: &std::io::Error) -> Row {
    let (class, message) = match e.kind() {
        std::io::ErrorKind::NotFound => (
            "Errno::ENOENT",
            format!("No such file or directory @ rb_sysopen - {path}"),
        ),
        std::io::ErrorKind::PermissionDenied => (
            "Errno::EACCES",
            format!("Permission denied @ rb_sysopen - {path}"),
        ),
        _ => ("IOError", e.to_string()),
    };
    let info = ErrInfo {
        class: class.into(),
        message,
    };
    let facts = Facts {
        path: path.into(),
        sha256: None,
        format: None,
        is_collection: false,
    };
    let mut row = facts.base(None, 0);
    row.errors.font_file = Some(font_file_error(&info));
    row.errors.collection_file = Some(load_error(&info));
    row
}

// ---- errors ---------------------------------------------------------------

fn info(e: &Error) -> ErrInfo {
    ErrInfo {
        class: e.class.ruby_name().into(),
        message: e.message.clone(),
    }
}

/// Ruby `Exception#inspect`: `#<Class: message>`, or the bare class when the
/// message is empty.
fn inspect(e: &ErrInfo) -> String {
    if e.message.is_empty() {
        e.class.clone()
    } else {
        format!("#<{}: {}>", e.class, e.message)
    }
}

/// `FontFile.raise_font_file_error`.
fn font_file_error(e: &ErrInfo) -> RowError {
    RowError {
        class: FONT_FILE_ERROR.into(),
        message: format!("Font file could not be parsed: {}.", inspect(e)),
        cause: Some(e.clone()),
    }
}

/// `CollectionFile` wraps every non-indexability failure this way.
fn load_error(e: &ErrInfo) -> RowError {
    RowError {
        class: FONT_FILE_ERROR.into(),
        message: format!("Font collection could not be loaded: {}", e.message),
        cause: Some(e.clone()),
    }
}

fn not_a_collection() -> RowError {
    RowError {
        class: FONT_FILE_ERROR.into(),
        message: format!("Font collection could not be loaded: {NOT_A_COLLECTION_TEXT}"),
        cause: Some(ErrInfo {
            class: FONT_FILE_ERROR.into(),
            message: NOT_A_COLLECTION_TEXT.into(),
        }),
    }
}

// ---- rows -----------------------------------------------------------------

fn load(
    bytes: &[u8],
    index: Option<usize>,
    mode: LoadingMode,
    lazy: bool,
) -> Result<Font<'_>, Error> {
    Font::load(bytes, index.unwrap_or(0), Options { mode, lazy })
}

/// A collection without faces has no row to hold what fontist raises for it
/// (the first face cannot be loaded), so it gets one of its own.
fn empty_collection_row(bytes: &[u8], facts: &mut Facts) -> Row {
    let mut row = facts.base(None, 0);
    if let Err(e) = load(bytes, Some(0), LoadingMode::Metadata, true) {
        row.errors.font_file = Some(font_file_error(&info(&e)));
    }
    row.errors.collection_file = first_face_error(bytes);
    row
}

fn face_row(bytes: &[u8], facts: &Facts, index: Option<usize>, num_faces: usize) -> Row {
    let mut row = facts.base(index, num_faces);
    // fontist reads a non-collection, or face 0 of a collection.
    let fontist_face = index.is_none_or(|i| i == 0);
    match load(bytes, index, LoadingMode::Metadata, true) {
        Err(e) => {
            if fontist_face {
                row.errors.font_file = Some(font_file_error(&info(&e)));
            }
        }
        Ok(font) => {
            if let Err(e) = fill_from_face(&mut row, &font, bytes, index) {
                if fontist_face {
                    row.errors.font_file = Some(font_file_error(&info(&e)));
                }
            }
        }
    }
    match index {
        None => row.errors.collection_file = Some(not_a_collection()),
        Some(i) => fill_collection_errors(&mut row, bytes, i),
    }
    row
}

/// FontFile flow: validate -> names (with fallback) -> fvar -> instances.
/// Partial results stay in the row when a later step fails, as in Ruby.
fn fill_from_face(
    row: &mut Row,
    font: &Font<'_>,
    bytes: &[u8],
    index: Option<usize>,
) -> Result<(), Error> {
    row.validation = Some(indexability::validate(font));
    let names = font.name()?;
    let map = names_with_fallback(names.as_ref());
    let family = map.iter().any(|(k, v)| *k == "family_name" && v.is_some());
    row.names = Some(map);
    row.has_fvar = font.has_table("fvar");
    if row.has_fvar && family {
        row.variable_instances = variable_instances(bytes, index);
    }
    Ok(())
}

/// CollectionFile flow: metadata, `lazy: false`, English names only; and the
/// first face's indexability, which gates the whole file.
fn fill_collection_errors(row: &mut Row, bytes: &[u8], index: usize) {
    match load(bytes, Some(index), LoadingMode::Metadata, false) {
        Err(e) => row.errors.collection_member = Some(info(&e)),
        Ok(member) => match member.name() {
            Err(e) => row.errors.collection_member = Some(info(&e)),
            Ok(table) => row.english_names = Some(english_names(table.as_ref())),
        },
    }
    row.errors.collection_file = first_face_error(bytes);
}

fn first_face_error(bytes: &[u8]) -> Option<RowError> {
    match load(bytes, Some(0), LoadingMode::Metadata, true) {
        Err(e) => Some(load_error(&info(&e))),
        Ok(first) => {
            let report = indexability::validate(&first);
            (!report.valid).then(|| RowError {
                class: INDEXABILITY_ERROR.into(),
                message: format!(
                    "Font collection failed indexability validation (first font): {}",
                    report.error_summary
                ),
                cause: None,
            })
        }
    }
}

// ---- names ----------------------------------------------------------------

fn english_names(table: Option<&Name>) -> NameMap {
    match table {
        None => Vec::new(),
        Some(n) => NAME_FIELDS
            .iter()
            .map(|&(k, id)| (k, n.english_name(id)))
            .collect(),
    }
}

fn names_with_fallback(table: Option<&Name>) -> NameMap {
    match table {
        None => Vec::new(),
        Some(n) => NAME_FIELDS
            .iter()
            .map(|&(k, id)| (k, n.english_name(id).or_else(|| any_name(n, id))))
            .collect(),
    }
}

/// fontist's `extract_any_name`: the first record of this id (any platform)
/// that decodes to a non-empty string.
fn any_name(n: &Name, id: u16) -> Option<String> {
    n.records
        .iter()
        .filter(|r| r.name_id == id)
        .find_map(|r| n.decode(r).filter(|s| !s.is_empty()))
}

// ---- variable instances ----------------------------------------------------

/// fvar data is gated out of metadata mode, so this is the second (full) load.
fn variable_instances(bytes: &[u8], index: Option<usize>) -> Vec<Instance> {
    let Ok(font) = load(bytes, index, LoadingMode::Full, true) else {
        return Vec::new();
    };
    instances_of(&font).unwrap_or_default()
}

fn instances_of(font: &Font<'_>) -> Result<Vec<Instance>, Error> {
    if !font.has_table("fvar") {
        return Ok(Vec::new());
    }
    let Some(fvar) = font.fvar()? else {
        return Ok(Vec::new());
    };
    let Some(n) = font.name()? else {
        return Ok(Vec::new());
    };
    let Some(base) = n
        .english_name(16)
        .or_else(|| any_name(&n, 16))
        .or_else(|| n.english_name(1))
    else {
        return Ok(Vec::new());
    };
    Ok(fvar
        .instances
        .iter()
        .filter_map(|inst| {
            let style = n
                .english_name(inst.name_id)
                .or_else(|| any_name(&n, inst.name_id))?;
            Some(if RIBBI.contains(&style.to_lowercase().as_str()) {
                Instance {
                    full_name: format!("{base} {style}"),
                    family: base.clone(),
                    style,
                }
            } else {
                let family = format!("{base} {style}");
                Instance {
                    full_name: family.clone(),
                    family,
                    style: "Regular".into(),
                }
            })
        })
        .collect())
}

// ---- JSON ---------------------------------------------------------------------

fn opt_str(s: &Option<String>) -> Json {
    s.as_ref().map_or(Json::Null, |s| Json::Str(s.clone()))
}

fn names_json(n: &Option<NameMap>) -> Json {
    n.as_ref().map_or(Json::Null, |m| {
        Json::Obj(m.iter().map(|(k, v)| (*k, opt_str(v))).collect())
    })
}

fn err_info_json(e: &ErrInfo) -> Json {
    Json::Obj(vec![
        ("class", Json::Str(e.class.clone())),
        ("message", Json::Str(e.message.clone())),
    ])
}

fn row_error_json(e: &RowError) -> Json {
    Json::Obj(vec![
        ("class", Json::Str(e.class.clone())),
        ("message", Json::Str(e.message.clone())),
        ("cause", e.cause.as_ref().map_or(Json::Null, err_info_json)),
    ])
}

fn validation_json(r: &IndexabilityReport) -> Json {
    Json::Obj(vec![
        ("profile", Json::Str("indexability".into())),
        ("valid", Json::Bool(r.valid)),
        ("status", Json::Str(r.status.into())),
        (
            "issues",
            Json::Arr(
                r.issues
                    .iter()
                    .map(|i| {
                        Json::Obj(vec![
                            ("severity", Json::Str(i.severity.into())),
                            ("category", Json::Str(i.category.into())),
                            ("message", Json::Str(i.message.clone())),
                            ("location", opt_str(&i.location)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("error_summary", Json::Str(r.error_summary.clone())),
    ])
}

impl Row {
    /// The row as the facade's scan row (an unsupported row has only
    /// `status` and the file-level fields).
    pub fn to_json(&self) -> Json {
        let status = Json::Str(
            match self.status {
                RowStatus::Ok => "ok",
                RowStatus::Unsupported => "unsupported",
            }
            .into(),
        );
        Json::Obj(vec![
            ("status", status),
            ("path", Json::Str(self.path.clone())),
            (
                "collection_index",
                self.collection_index
                    .map_or(Json::Null, |i| Json::Int(i as i64)),
            ),
            ("sha256", opt_str(&self.sha256)),
            ("format", opt_str(&self.format)),
            ("is_collection", Json::Bool(self.is_collection)),
            ("num_faces", Json::Int(self.num_faces as i64)),
            ("has_fvar", Json::Bool(self.has_fvar)),
            ("names", names_json(&self.names)),
            ("english_names", names_json(&self.english_names)),
            (
                "variable_instances",
                Json::Arr(
                    self.variable_instances
                        .iter()
                        .map(|i| {
                            Json::Obj(vec![
                                ("family", Json::Str(i.family.clone())),
                                ("style", Json::Str(i.style.clone())),
                                ("full_name", Json::Str(i.full_name.clone())),
                            ])
                        })
                        .collect(),
                ),
            ),
            (
                "validation",
                self.validation.as_ref().map_or(Json::Null, validation_json),
            ),
            (
                "errors",
                Json::Obj(vec![
                    (
                        "font_file",
                        self.errors
                            .font_file
                            .as_ref()
                            .map_or(Json::Null, row_error_json),
                    ),
                    (
                        "collection_file",
                        self.errors
                            .collection_file
                            .as_ref()
                            .map_or(Json::Null, row_error_json),
                    ),
                    (
                        "collection_member",
                        self.errors
                            .collection_member
                            .as_ref()
                            .map_or(Json::Null, err_info_json),
                    ),
                ]),
            ),
        ])
    }
}

/// Extensions `scan` / `audit_dir` pick up in a directory (case-insensitive).
pub const FONT_EXTENSIONS: [&str; 9] = [
    "ttf", "otf", "ttc", "otc", "woff", "woff2", "dfont", "pfb", "pfa",
];

/// Font files under `root`, recursively, sorted by path (byte order). A file
/// `root` yields itself. Directory symlinks are not followed; non-font
/// extensions are skipped.
pub fn font_files(root: &std::path::Path) -> std::io::Result<Vec<std::path::PathBuf>> {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let kind = entry.file_type()?;
            if kind.is_dir() {
                walk(&path, out)?;
            } else if (kind.is_file() || (kind.is_symlink() && path.is_file()))
                && path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| FONT_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
            {
                out.push(path);
            }
        }
        Ok(())
    }
    if !root.is_dir() {
        return Ok(vec![root.to_path_buf()]);
    }
    let mut out = Vec::new();
    walk(root, &mut out)?;
    out.sort();
    Ok(out)
}

/// `scan` over a file or a whole directory: every row of every font file.
pub fn scan_dir(root: &std::path::Path) -> std::io::Result<Vec<Row>> {
    Ok(font_files(root)?
        .iter()
        .flat_map(|p| scan_path(p))
        .collect())
}
