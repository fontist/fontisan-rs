//! A stable, machine-readable error kind for every engine error.
//!
//! The engine reports Ruby class names and messages because the Ruby tier is the
//! contract. A caller that maps engine errors onto its own exception hierarchy should
//! switch on the [`ErrorKind`], not parse the message. The numeric values are part of
//! the C ABI (`fontisan_result.error_kind`, `FONTISAN_KIND_*`): append only, never renumber.

/// What went wrong, independent of wording. `repr(i32)` values are ABI.
#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    /// No error (a successful call).
    None = 0,
    /// Not a readable font, or a structure inside it is malformed
    /// (`Fontisan::InvalidFontError`, `RuntimeError "Invalid ... file"`).
    InvalidFont = 1,
    /// An axis tag the font does not have (`Fontisan::InvalidAxisError`).
    InvalidAxis = 2,
    /// A coordinate outside the axis range, or not finite (`Fontisan::InvalidCoordinatesError`).
    InvalidCoordinates = 3,
    /// A file-system or other I/O failure (`Errno::*`).
    Io = 4,
    /// A format or feature the engine does not serve; the caller serves it (`fontisan::Unsupported`).
    Unsupported = 5,
    /// Subsetting failed (`Fontisan::SubsettingError`).
    Subsetting = 6,
    /// A table's contents are corrupt (`Fontisan::CorruptedTableError`).
    CorruptedTable = 7,
    /// A table the operation needs is absent (`Fontisan::MissingTableError`).
    MissingTable = 8,
    /// A bad argument from the caller (`ArgumentError`).
    InvalidArgument = 9,
    /// Data ended early (`EOFError`, `IOError "data truncated"`).
    Truncated = 10,
    /// Anything else (`Fontisan::Error`, engine panic, ...). Use the message.
    Other = 11,
}

impl ErrorKind {
    /// The kind for a Ruby class name as the engine reports it.
    pub fn from_class(class: &str) -> Self {
        match class {
            "Fontisan::InvalidFontError" | "RuntimeError" => Self::InvalidFont,
            "Fontisan::InvalidAxisError" => Self::InvalidAxis,
            "Fontisan::InvalidCoordinatesError" => Self::InvalidCoordinates,
            "Fontisan::UnsupportedFormat" | "fontisan::Unsupported" => Self::Unsupported,
            "Fontisan::SubsettingError" => Self::Subsetting,
            "Fontisan::CorruptedTableError" => Self::CorruptedTable,
            "Fontisan::MissingTableError" | "Fontisan::MissingVariationTableError" => {
                Self::MissingTable
            }
            "ArgumentError" => Self::InvalidArgument,
            "EOFError" | "IOError" | "Fontisan::Reference::TableDigests::Truncated" => {
                Self::Truncated
            }
            c if c.starts_with("Errno::") => Self::Io,
            _ => Self::Other,
        }
    }

    /// The ABI value.
    pub fn code(self) -> i32 {
        self as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes_map_to_kinds() {
        for (class, kind) in [
            ("Fontisan::InvalidAxisError", ErrorKind::InvalidAxis),
            (
                "Fontisan::InvalidCoordinatesError",
                ErrorKind::InvalidCoordinates,
            ),
            ("Fontisan::InvalidFontError", ErrorKind::InvalidFont),
            ("Fontisan::SubsettingError", ErrorKind::Subsetting),
            ("Fontisan::CorruptedTableError", ErrorKind::CorruptedTable),
            ("Fontisan::MissingTableError", ErrorKind::MissingTable),
            ("fontisan::Unsupported", ErrorKind::Unsupported),
            ("Errno::EACCES", ErrorKind::Io),
            ("EOFError", ErrorKind::Truncated),
            ("Fontisan::Error", ErrorKind::Other),
        ] {
            assert_eq!(ErrorKind::from_class(class), kind, "{class}");
        }
    }
}
