//! Read speed on a large CFF2 collection (fontisan#156 P3 perf gate). Needs the
//! fetched font: set `FONTISAN_LARGE_FONT` (NotoSerifCJK-VF.otf.ttc); the test
//! returns when it is unset. `FONTISAN_LARGE_OUT=<dir>` also writes each
//! mode/lazy document there as `<mode>_<lazy>.json` for byte comparison.
use fontisan::json::Json;
use fontisan::read::read;
use fontisan::sfnt::{LoadingMode, Options};
use std::time::Instant;

#[test]
fn large_collection_read_is_fast() {
    let Ok(path) = std::env::var("FONTISAN_LARGE_FONT") else {
        return;
    };
    let bytes = std::fs::read(path).expect("font");
    let out = std::env::var("FONTISAN_LARGE_OUT").ok();
    for (name, mode, lazy) in [
        ("metadata_lazy", LoadingMode::Metadata, true),
        ("metadata_eager", LoadingMode::Metadata, false),
        ("full_lazy", LoadingMode::Full, true),
        ("full_eager", LoadingMode::Full, false),
    ] {
        let t = Instant::now();
        let faces = read(&bytes, Options { mode, lazy }).expect("read");
        let secs = t.elapsed().as_secs_f64();
        eprintln!("read {name}: {secs:.3}s");
        if let Some(dir) = &out {
            let doc = Json::Arr(faces).render();
            std::fs::write(format!("{dir}/{name}.json"), doc).unwrap();
        }
        // Ruby needs 1.78 s (lazy) and 10.97 s (eager) including ~1 s of boot.
        let limit = if mode == LoadingMode::Metadata {
            1.0
        } else {
            9.0
        };
        // Only a release build is comparable; a debug build is ~15x slower.
        assert!(cfg!(debug_assertions) || secs < limit, "{name}: {secs:.3}s");
    }
}
