//! Errors that reproduce the Ruby reference's error surface: the Ruby class
//! name, the message, and the cause chain (#159, #160: fontist shows these).

/// The Ruby exception class an error stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorClass {
    /// `RuntimeError` (Ruby wraps an EOF during the directory read in one).
    Runtime,
    /// `EOFError`: a field was read with no bytes left.
    Eof,
    /// `IOError` ("data truncated"): a field was read with only some of its bytes left.
    Io,
    /// `Fontisan::InvalidFontError`.
    InvalidFont,
    /// `ArgumentError` (table gated out by the loading mode).
    Argument,
    /// `NoMethodError` (Ruby's batched metadata read hits a nil buffer).
    NoMethod,
    /// `Fontisan::CorruptedTableError` (loca and CFF structure checks).
    CorruptedTable,
    /// `Fontisan::Reference::TableDigests::Truncated` (the CFF2 charstring walk).
    Truncated,
    /// A format the native engine does not read (WOFF/WOFF2 before 1e, dfont,
    /// Type 1): the caller routes it elsewhere. Not a Ruby class.
    Unsupported,
}

impl ErrorClass {
    /// The Ruby class name, as it appears in goldens.
    pub fn ruby_name(self) -> &'static str {
        match self {
            Self::Runtime => "RuntimeError",
            Self::Eof => "EOFError",
            Self::Io => "IOError",
            Self::InvalidFont => "Fontisan::InvalidFontError",
            Self::Argument => "ArgumentError",
            Self::NoMethod => "NoMethodError",
            Self::CorruptedTable => "Fontisan::CorruptedTableError",
            Self::Truncated => "Fontisan::Reference::TableDigests::Truncated",
            Self::Unsupported => "Fontisan::UnsupportedFormat",
        }
    }
}

impl ErrorClass {
    /// The machine-readable kind of this class.
    pub fn kind(self) -> crate::ErrorKind {
        crate::ErrorKind::from_class(self.ruby_name())
    }
}

/// A read failure: class, message and the (at most one) cause.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub class: ErrorClass,
    pub message: String,
    pub cause: Option<(ErrorClass, String)>,
}

impl Error {
    /// The machine-readable kind; switch on this, not on the message.
    pub fn kind(&self) -> crate::ErrorKind {
        self.class.kind()
    }
    pub(crate) fn new(class: ErrorClass, message: impl Into<String>) -> Self {
        Self {
            class,
            message: message.into(),
            cause: None,
        }
    }
    pub(crate) fn eof() -> Self {
        Self::new(ErrorClass::Eof, "End of file reached")
    }
    pub(crate) fn truncated() -> Self {
        Self::new(ErrorClass::Io, "data truncated")
    }
    /// Ruby's `load_*` rewraps an EOF as `RuntimeError "Invalid <Class> file: ..."`.
    pub(crate) fn wrap_load(self, font_type: &str) -> Self {
        if self.class != ErrorClass::Eof {
            return self;
        }
        Self {
            class: ErrorClass::Runtime,
            message: format!("Invalid {font_type} file: {}", self.message),
            cause: Some((self.class, self.message)),
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.class.ruby_name(), self.message)
    }
}

impl std::error::Error for Error {}
