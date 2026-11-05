//! Audit checks and report (card 1f). `types` is the shared contract; each check
//! lives in `checks/<name>.rs` as `pub fn run(font) -> Result<Vec<Issue>, CheckError>`.

pub mod checks;
pub mod types;

pub use types::{CheckError, CheckFn, Issue, Severity};
