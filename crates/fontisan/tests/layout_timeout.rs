//! Regression: a libFuzzer `font_layout` timeout (13 s on a 185 KB input).
//! `scripts()` x `features()` re-parsed the ScriptList per script and the
//! LangSys / feature de-duplication was quadratic. The input is committed as
//! `fuzz/seeds/timeout-font-layout-quadratic.otf` and goes through the very
//! same body as the fuzz target.
#[path = "../../../fuzz/bodies.rs"]
mod bodies;

use std::time::{Duration, Instant};

#[test]
fn font_layout_timeout_input_is_fast() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fuzz/seeds/timeout-font-layout-quadratic.otf");
    let data = std::fs::read(path).unwrap();
    let start = Instant::now();
    bodies::font_layout(&data);
    let took = start.elapsed();
    assert!(took < Duration::from_secs(1), "font_layout took {took:?}");
}
