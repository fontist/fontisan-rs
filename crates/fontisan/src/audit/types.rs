//! Shared audit types, mirroring the Ruby reference's audit report
//! (`Fontisan::Commands::AuditCommand`, canonical JSON in fontisan's
//! docs/reference-facade.md). Every check is a pure function over a loaded face.

use crate::sfnt::Font;

/// Issue severity, serialised lowercase as in the Ruby report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Info,
    Warning,
    Error,
    Fatal,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Error => "error",
            Severity::Fatal => "fatal",
        }
    }
}

/// One validation issue: `{category, location?, message, severity}`.
/// `location: None` is omitted from the JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub category: String,
    pub location: Option<String>,
    pub message: String,
    pub severity: Severity,
}

impl Issue {
    pub fn new(category: &str, severity: Severity, message: impl Into<String>) -> Self {
        Issue {
            category: category.into(),
            location: None,
            message: message.into(),
            severity,
        }
    }

    pub fn at(mut self, location: impl Into<String>) -> Self {
        self.location = Some(location.into());
        self
    }
}

/// A check that raised in Ruby. The runner turns it into a fatal issue
/// `"Check <code> failed to execute: <class>: <message>"` with no location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckError {
    pub class: String,
    pub message: String,
}

impl CheckError {
    pub fn new(class: &str, message: impl Into<String>) -> Self {
        CheckError {
            class: class.into(),
            message: message.into(),
        }
    }

    pub fn into_issue(self, code: &str) -> Issue {
        Issue::new(
            code,
            Severity::Fatal,
            format!(
                "Check {code} failed to execute: {}: {}",
                self.class, self.message
            ),
        )
    }
}

/// Signature every check implements. The `&'static str` in a registry entry is
/// the Ruby check code (the issue `category` used for a fatal wrap).
pub type CheckFn = fn(&Font<'_>) -> Result<Vec<Issue>, CheckError>;
