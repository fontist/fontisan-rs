//! Read-side ops (card 1g-abi): `read`, `audit`, `scan`, `audit_dir`.
//!
//! Payload of every op ("packed rows"): u32 BE row count, then per row a u32 BE
//! byte length and that many bytes of UTF-8 canonical JSON. `read` and `audit`
//! carry one row per face; `scan` one row per scan row; `audit_dir` one row per
//! font file. A failed `read` / `audit` is an error result: status
//! `ReadFailed` with the message `{"class","message","causes":[...]}` (JSON), or
//! `Unsupported` (dfont, Type 1) so the caller serves the file itself.
use super::{FfiError, FontisanStatus};
use crate::json::Json;
use crate::sfnt::{Error, ErrorClass, LoadingMode, Options};
use std::path::Path;

/// `flags` bit of [`super::OP_READ`]: `mode: :metadata` (clear = `:full`).
pub const READ_FLAG_METADATA: u32 = 1;
/// `flags` bit of [`super::OP_READ`]: `lazy: true` (clear = eager).
pub const READ_FLAG_LAZY: u32 = 2;
/// `flags` bit of [`super::OP_AUDIT`] and [`super::OP_AUDIT_DIR`]: run the validation checks.
pub const AUDIT_FLAG_VALIDATE: u32 = 1;

pub fn pack(rows: &[Json]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(rows.len() as u32).to_be_bytes());
    for row in rows {
        let text = row.render();
        out.extend_from_slice(&(text.len() as u32).to_be_bytes());
        out.extend_from_slice(text.as_bytes());
    }
    out
}

fn failed(e: &Error) -> FfiError {
    if e.class == ErrorClass::Unsupported {
        return FfiError::new(FontisanStatus::Unsupported, e.to_string());
    }
    FfiError::new(
        FontisanStatus::ReadFailed,
        crate::read::error_json(e).render(),
    )
    .with_kind(e.kind())
}

pub fn read(input: &[u8], flags: u32) -> Result<Vec<u8>, FfiError> {
    let options = Options {
        mode: if flags & READ_FLAG_METADATA != 0 {
            LoadingMode::Metadata
        } else {
            LoadingMode::Full
        },
        lazy: flags & READ_FLAG_LAZY != 0,
    };
    crate::read::read(input, options)
        .map(|rows| pack(&rows))
        .map_err(|e| failed(&e))
}

/// `input` = u32 BE name length, the source file name, the font bytes.
pub fn audit(input: &[u8], flags: u32) -> Result<Vec<u8>, FfiError> {
    let bad = |m: &str| FfiError::new(FontisanStatus::InvalidArgument, m);
    let len = input
        .get(..4)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize)
        .ok_or_else(|| bad("audit input needs a 4-byte source-name length"))?;
    let name = input
        .get(4..4 + len)
        .and_then(|b| std::str::from_utf8(b).ok())
        .ok_or_else(|| bad("audit source name is truncated or not UTF-8"))?;
    let reports =
        crate::audit::audit_staged(&input[4 + len..], name, flags & AUDIT_FLAG_VALIDATE != 0)
            .map_err(|(e, stage)| {
                if e.class == ErrorClass::Unsupported {
                    return failed(&e);
                }
                FfiError::new(
                    FontisanStatus::ReadFailed,
                    crate::audit::command_error_json(&e, stage).render(),
                )
                .with_kind(e.kind())
            })?;
    Ok(pack(
        &reports.iter().map(|r| r.to_json()).collect::<Vec<_>>(),
    ))
}

fn path_arg(input: &[u8]) -> Result<&Path, FfiError> {
    std::str::from_utf8(input)
        .map(Path::new)
        .map_err(|_| FfiError::new(FontisanStatus::InvalidArgument, "path is not UTF-8"))
}

fn walk_failed(path: &Path, e: &std::io::Error) -> FfiError {
    FfiError::new(
        FontisanStatus::InvalidArgument,
        format!("cannot read {}: {e}", path.display()),
    )
    .with_kind(crate::ErrorKind::Io)
}

/// `input` = a UTF-8 file or directory path.
pub fn scan(input: &[u8]) -> Result<Vec<u8>, FfiError> {
    let path = path_arg(input)?;
    let rows = crate::scan::scan_dir(path).map_err(|e| walk_failed(path, &e))?;
    Ok(pack(&rows.iter().map(|r| r.to_json()).collect::<Vec<_>>()))
}

/// `input` = a UTF-8 file or directory path.
pub fn audit_dir(input: &[u8], flags: u32) -> Result<Vec<u8>, FfiError> {
    let path = path_arg(input)?;
    let files = crate::scan::font_files(path).map_err(|e| walk_failed(path, &e))?;
    let validate = flags & AUDIT_FLAG_VALIDATE != 0;
    Ok(pack(
        &files
            .iter()
            .map(|f| crate::audit::dir_row(f, validate))
            .collect::<Vec<_>>(),
    ))
}
