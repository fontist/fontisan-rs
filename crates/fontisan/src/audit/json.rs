//! The canonical audit JSON: nil and empty collections omitted, except the
//! fields that are always present (`axes`, `font_revision_raw`,
//! `total_glyphs`, ...; `total_codepoints` only when the font has a `cmap`). Parity-tested against the audit
//! goldens through the C ABI (`tests/ffi_read_ops.rs`).
use super::report::{Report, Summary};
use super::types::Issue;
use crate::json::{Json, obj};

fn issue_json(i: &Issue) -> Json {
    let mut o = vec![("category", Json::from(i.category.clone()))];
    if let Some(l) = &i.location {
        o.push(("location", Json::from(l.clone())));
    }
    o.push(("message", Json::from(i.message.clone())));
    o.push(("severity", Json::from(i.severity.as_str())));
    Json::Obj(o)
}

fn summary_json(s: &Summary) -> Json {
    obj! {"error_count" => s.error_count, "fatal_count" => s.fatal_count,
    "info_count" => s.info_count, "passed" => s.passed, "total" => s.total,
    "warning_count" => s.warning_count}
}

impl Report {
    pub fn to_json(&self) -> Json {
        let mut o: Vec<(&'static str, Json)> = Vec::new();
        o.push(("source_file", self.source_file.clone().into()));
        o.push(("source_sha256", self.source_sha256.clone().into()));
        o.push(("source_format", self.source_format.clone().into()));
        if let Some(i) = self.font_index {
            o.push(("font_index", i.into()));
        }
        o.push(("num_fonts_in_source", self.num_fonts_in_source.into()));
        for (k, v) in [
            ("family_name", &self.family_name),
            ("subfamily_name", &self.subfamily_name),
            ("full_name", &self.full_name),
            ("postscript_name", &self.postscript_name),
            ("version", &self.version),
            ("panose", &self.panose),
        ] {
            if let Some(v) = v {
                o.push((k, v.clone().into()));
            }
        }
        o.push(("font_revision_raw", self.font_revision_raw.into()));
        o.push(("is_variable", self.is_variable.into()));
        o.push(("italic", self.italic.into()));
        o.push(("bold", self.bold.into()));
        if let Some(w) = self.weight_class {
            o.push(("weight_class", w.into()));
        }
        if let Some(w) = self.width_class {
            o.push(("width_class", w.into()));
        }
        o.push((
            "axes",
            Json::Arr(
                self.axes
                    .iter()
                    .map(|a| {
                        obj! {"tag" => a.tag.clone(), "min_value_raw" => a.min_value_raw,
                        "default_value_raw" => a.default_value_raw,
                        "max_value_raw" => a.max_value_raw, "name_id" => a.name_id}
                    })
                    .collect(),
            ),
        ));
        if let Some(n) = self.total_codepoints {
            o.push(("total_codepoints", n.into()));
        }
        if !self.cmap_subtables.is_empty() {
            o.push(("cmap_subtables", self.cmap_subtables.clone().into()));
        }
        o.push(("total_glyphs", self.total_glyphs.into()));
        if !self.opentype_scripts.is_empty() {
            o.push(("opentype_scripts", self.opentype_scripts.clone().into()));
        }
        if !self.features.is_empty() {
            o.push(("features", self.features.clone().into()));
        }
        if let Some(issues) = self.validation_issues.as_ref().filter(|i| !i.is_empty()) {
            o.push((
                "validation_issues",
                Json::Arr(issues.iter().map(issue_json).collect()),
            ));
        }
        if let Some(s) = &self.validation_summary {
            o.push(("validation_summary", summary_json(s)));
        }
        Json::Obj(o)
    }
}

/// The error `AuditCommand` raises: `BaseCommand#load_font` wraps any load
/// failure but format errors in `Fontisan::Error "Failed to load font: ..."`
/// (cause chain: the original, then its own cause); everything else is as raised.
pub fn command_error_json(e: &crate::sfnt::Error, stage: super::Stage) -> Json {
    use crate::sfnt::ErrorClass::{InvalidFont, Unsupported};
    if stage != super::Stage::Load || matches!(e.class, InvalidFont | Unsupported) {
        return crate::read::error_json(e);
    }
    let causes: Vec<Json> = std::iter::once((e.class, &e.message))
        .chain(e.cause.iter().map(|(c, m)| (*c, m)))
        .map(|(c, m)| obj! {"class" => c.ruby_name(), "message" => m.clone()})
        .collect();
    obj! {
        "class" => "Fontisan::Error",
        "message" => format!("Failed to load font: {}", e.message),
        "causes" => causes,
    }
}

/// One `audit_dir` row: `{path, status: ok|unsupported|error, reports | error}`.
/// `unsupported` (dfont, Type 1 pfb) is for the caller to serve elsewhere; any
/// other failure is a row, never an abort of the directory.
pub fn dir_row(path: &std::path::Path, validate: bool) -> Json {
    use crate::sfnt::ErrorClass;
    let name = path.to_string_lossy().into_owned();
    let source = path
        .file_name()
        .map_or_else(|| name.clone(), |n| n.to_string_lossy().into_owned());
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            let class = if e.kind() == std::io::ErrorKind::NotFound {
                "Errno::ENOENT"
            } else {
                "IOError"
            };
            return Json::Obj(vec![
                ("path", name.clone().into()),
                ("status", "error".into()),
                (
                    "error",
                    obj! {"class" => class, "message" => e.to_string(), "causes" => Json::Arr(vec![])},
                ),
            ]);
        }
    };
    match super::audit_staged(&bytes, &source, validate) {
        Ok(reports) => obj! {
            "path" => name,
            "status" => "ok",
            "reports" => reports.iter().map(Report::to_json).collect::<Vec<_>>(),
        },
        Err((e, _)) if e.class == ErrorClass::Unsupported => {
            obj! {"path" => name, "status" => "unsupported"}
        }
        Err((e, stage)) => Json::Obj(vec![
            ("path", name.clone().into()),
            ("status", "error".into()),
            ("error", command_error_json(&e, stage)),
        ]),
    }
}
