//! `ot_post` (Ruby `PostCheck`).
use crate::audit::ruby::{fixed, float_to_s, need};
use crate::audit::types::{CheckError, Issue, Severity};
use crate::sfnt::Font;

pub const CODE: &str = "ot_post";

pub fn run(font: &Font<'_>) -> Result<Vec<Issue>, CheckError> {
    if !font.has_table("post") {
        return Ok(vec![]);
    }
    let post = need(font.post(), "version")?;
    let head = if font.has_table("head") {
        font.head().map_err(crate::audit::ruby::check_error)?
    } else {
        None
    };
    let mut v = Vec::new();
    let version = fixed(post.version_raw);
    if ![1.0, 2.0, 2.5, 3.0, 4.0].contains(&version) {
        v.push(
            Issue::new(
                CODE,
                Severity::Error,
                format!(
                    "post.version is {} but must be one of 1.0, 2.0, 2.5, 3.0, 4.0",
                    float_to_s(version)
                ),
            )
            .at("post.version"),
        );
    }
    let angle = fixed(post.italic_angle_raw);
    if let Some(head) = head {
        let italic = head.mac_style & 0x02 != 0;
        if italic && angle >= 0.0 {
            v.push(
                Issue::new(
                    CODE,
                    Severity::Warning,
                    format!(
                        "post.italicAngle is {} but head.macStyle indicates italic (angle should be negative)",
                        float_to_s(angle)
                    ),
                )
                .at("post.italic_angle"),
            );
        } else if !italic && angle != 0.0 {
            v.push(
                Issue::new(
                    CODE,
                    Severity::Info,
                    format!(
                        "post.italicAngle is {} but head.macStyle does not indicate italic (angle should be 0)",
                        float_to_s(angle)
                    ),
                )
                .at("post.italic_angle"),
            );
        }
    }
    if post.is_fixed_pitch > 1 {
        v.push(
            Issue::new(
                CODE,
                Severity::Warning,
                format!(
                    "post.isFixedPitch is {} but must be 0 (proportional) or 1 (monospace)",
                    post.is_fixed_pitch
                ),
            )
            .at("post.is_fixed_pitch"),
        );
    }
    Ok(v)
}
