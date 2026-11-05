//! Variation tables (card 1d-var): fvar axes and named instances, avar segment
//! maps, and the parsed digests of gvar, HVAR, VVAR, MVAR and STAT that the
//! reference facade stores (`docs/reference-facade.md`, "Parsed table digests").
//!
//! The digests are defined on the raw table bytes, so they are computed from the
//! bytes directly (see `digests`); read-fonts adds nothing for them. fvar keeps
//! the Ruby parser's quirks (see `fvar`).
mod avar;
mod digests;
mod fvar;

pub use avar::Avar;
pub use digests::{
    AxisValue, DigestFailure, GvarDigest, HvarDigest, IvsDigest, MapDigest, MvarDigest, MvarRecord,
    StatAxis, StatDigest,
};
pub use fvar::{Fvar, FvarAxis, FvarInstance};

use super::{Error, Font};

/// What the facade does for a digest: absent or mode-gated tables are `None`,
/// a structure running past its table is a recorded failure.
type Digested<T> = Result<Option<T>, DigestFailure>;

impl Font<'_> {
    /// fvar axes and named instances (`Fontisan::Reference` `fvar`).
    pub fn fvar(&self) -> Result<Option<Fvar>, Error> {
        self.table_data("fvar").map(Fvar::parse).transpose()
    }
    /// avar segment maps.
    pub fn avar(&self) -> Result<Option<Avar>, Error> {
        self.table_data("avar").map(Avar::parse).transpose()
    }
    pub fn gvar_digest(&self) -> Digested<GvarDigest> {
        self.table_data("gvar").map(digests::gvar).transpose()
    }
    pub fn hvar_digest(&self) -> Digested<HvarDigest> {
        self.table_data("HVAR").map(digests::hvar).transpose()
    }
    pub fn vvar_digest(&self) -> Digested<HvarDigest> {
        self.table_data("VVAR").map(digests::hvar).transpose()
    }
    pub fn mvar_digest(&self) -> Digested<MvarDigest> {
        self.table_data("MVAR").map(digests::mvar).transpose()
    }
    pub fn stat_digest(&self) -> Digested<StatDigest> {
        self.table_data("STAT").map(digests::stat).transpose()
    }
}
