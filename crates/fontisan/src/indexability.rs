//! The `indexability` validation profile (Ruby `Validators::ProfileLoader` ->
//! `BasicValidator`, loading mode `metadata`): eight checks over name/head/maxp/
//! hhea, wrapped the way `Validators::Validator` wraps them. It is not a flat rule
//! list: a table that cannot be read becomes a `check_execution` error, a missing
//! one a `table_presence` error, a failed predicate a `table_validation` error.
use crate::sfnt::{Error, Font};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexIssue {
    /// `"error"` or `"warning"` (Ruby `add_error` / `add_warning`).
    pub severity: &'static str,
    pub category: &'static str,
    pub message: String,
    pub location: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexabilityReport {
    pub valid: bool,
    /// `invalid`, `valid_with_warnings` or `valid`.
    pub status: &'static str,
    pub issues: Vec<IndexIssue>,
    /// `"category: message"` of each error, joined with `"; "`.
    pub error_summary: String,
}

fn exception(e: &Error) -> IndexIssue {
    IndexIssue {
        severity: "error",
        category: "check_execution",
        message: format!(
            "Exception during check execution: {} - {}",
            e.class.ruby_name(),
            e.message
        ),
        location: None,
    }
}

/// `^[A-Za-z0-9-]+$` with Ruby's line anchors: some line must match whole.
fn postscript_valid(name: &str) -> bool {
    name.split('\n')
        .any(|l| !l.is_empty() && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
}

/// One table check: `severity` is the check's declared severity.
fn table_check<T>(
    out: &mut Vec<IndexIssue>,
    tag: &'static str,
    severity: &'static str,
    table: Result<Option<T>, Error>,
    pred: impl Fn(&T) -> bool,
) {
    match table {
        Err(e) => out.push(exception(&e)),
        Ok(None) => out.push(IndexIssue {
            severity,
            category: "table_presence",
            message: format!("Required table '{tag}' is missing"),
            location: Some(tag.into()),
        }),
        Ok(Some(t)) if !pred(&t) => out.push(IndexIssue {
            severity,
            category: "table_validation",
            message: format!("Table '{tag}' failed validation"),
            location: Some(tag.into()),
        }),
        Ok(Some(_)) => {}
    }
}

/// Validate a face (loaded in `metadata` mode by the caller).
pub fn validate(font: &Font<'_>) -> IndexabilityReport {
    let mut issues = Vec::new();
    // required_tables (structure): `all?` short-circuits on the first nil/raise.
    let structure = (|| -> Result<bool, Error> {
        Ok(font.name()?.is_some()
            && font.head()?.is_some()
            && font.maxp()?.is_some()
            && font.hhea()?.is_some())
    })();
    match structure {
        Err(e) => issues.push(exception(&e)),
        Ok(false) => issues.push(IndexIssue {
            severity: "error",
            category: "structure",
            message: "Font structure validation failed for check 'required_tables'".into(),
            location: None,
        }),
        Ok(true) => {}
    }
    table_check(&mut issues, "name", "error", font.name(), |n| n.format <= 1);
    table_check(&mut issues, "name", "error", font.name(), |n| {
        n.english_name(1).is_some_and(|s| !s.is_empty())
    });
    table_check(&mut issues, "name", "error", font.name(), |n| {
        n.english_name(6).is_some_and(|s| postscript_valid(&s))
    });
    table_check(&mut issues, "head", "error", font.head(), |h| {
        h.magic_number == 0x5F0F_3CF5
    });
    table_check(&mut issues, "head", "error", font.head(), |h| {
        (16..=16384).contains(&h.units_per_em)
    });
    table_check(&mut issues, "maxp", "error", font.maxp(), |m| {
        m.num_glyphs >= 1
    });
    table_check(&mut issues, "maxp", "warning", font.maxp(), |m| {
        m.v1.is_none_or(|f| f[0] <= 50000 && f[1] <= 10000 && f[9] <= 1000)
    });
    let errors: Vec<_> = issues.iter().filter(|i| i.severity == "error").collect();
    let error_summary = errors
        .iter()
        .map(|i| format!("{}: {}", i.category, i.message))
        .collect::<Vec<_>>()
        .join("; ");
    let status = if !errors.is_empty() {
        "invalid"
    } else if issues.iter().any(|i| i.severity == "warning") {
        "valid_with_warnings"
    } else {
        "valid"
    };
    IndexabilityReport {
        valid: errors.is_empty(),
        status,
        error_summary,
        issues,
    }
}
