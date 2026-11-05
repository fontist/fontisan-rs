//! The sfnt core and metadata model (card 1c): table directory, TTC/OTC, and
//! head, hhea, maxp, OS/2, post, name and cmap, with the loading modes and the
//! Ruby reference's lazy failure points. [`Font::from_bytes`] is the entry the
//! WOFF/WOFF2 decoders (1e) feed with decoded sfnt bytes.
mod cmap;
mod cursor;
mod error;
mod font;
mod layout;
mod outlines;
mod standard_names;
mod tables;
mod variations;
mod woff2_view;

pub use cmap::Cmap;
pub use error::{Error, ErrorClass};
pub use font::{
    Font, Format, LoadingMode, METADATA_TABLES, Options, TableEntry, TableSummary, detect_format,
    face_count,
};
pub use layout::{LayoutTable, Tag};
pub use outlines::{OutlineKind, OutlineSummary, Outlines};
pub use tables::{Head, Hhea, Maxp, Name, NameRecord, Os2, Post, Snapshot, Value};
pub use variations::{
    Avar, AxisValue, DigestFailure, Fvar, FvarAxis, FvarInstance, GvarDigest, HvarDigest,
    IvsDigest, MapDigest, MvarDigest, MvarRecord, StatAxis, StatDigest,
};
