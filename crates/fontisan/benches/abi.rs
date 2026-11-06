//! Criterion benches over the C ABI. `echo` measures the call overhead; `scan_dir`,
//! `audit_dir` and `read_per_file` measure the batch-first claim (one call over a whole
//! directory against one call per file) on the corpus fonts. The corpus benches are
//! skipped, with a message, when the corpus is not fetched (`cargo xtask corpus-fetch`,
//! or `FONTISAN_CORPUS_DIR`).
use criterion::{Criterion, criterion_group, criterion_main};
use fontisan::ffi::*;
use std::hint::black_box;
use std::path::PathBuf;
use std::ptr;

fn call(op: u32, flags: u32, input: &[u8]) -> i32 {
    let req = FontisanRequest {
        header: FontisanHeader::current::<FontisanRequest>(),
        op,
        flags,
        input: input.as_ptr(),
        input_len: input.len(),
    };
    let mut out = ptr::null_mut();
    let status = unsafe { fontisan_call(&req, &mut out) };
    unsafe { fontisan_result_free(out) };
    status
}

fn fonts_dir() -> Option<PathBuf> {
    let root = std::env::var_os("FONTISAN_CORPUS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.corpus/fontisan")
        });
    let dir = root.join("corpus/fonts");
    dir.is_dir().then_some(dir)
}

fn corpus(c: &mut Criterion) {
    let Some(dir) = fonts_dir() else {
        eprintln!("SKIPPED corpus benches: corpus fonts not found (cargo xtask corpus-fetch)");
        return;
    };
    let path = dir.to_str().unwrap().as_bytes().to_vec();
    let mut files: Vec<(String, Vec<u8>)> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| matches!(x, "ttf" | "otf" | "woff" | "woff2"))
        })
        .map(|p| (p.display().to_string(), std::fs::read(&p).unwrap()))
        .collect();
    files.sort();
    let mut g = c.benchmark_group("corpus");
    g.sample_size(10);
    g.bench_function("scan_dir_one_call", |b| {
        b.iter(|| black_box(call(OP_SCAN, 0, &path)))
    });
    g.bench_function("audit_dir_one_call", |b| {
        b.iter(|| black_box(call(OP_AUDIT_DIR, 0, &path)))
    });
    g.bench_function("read_per_file", |b| {
        b.iter(|| {
            for (_, bytes) in &files {
                black_box(call(OP_READ, READ_FLAG_METADATA, bytes));
            }
        })
    });
    g.finish();
}

fn echo(c: &mut Criterion) {
    let data = vec![7u8; 4096];
    c.bench_function("ffi_echo_4k", |b| {
        b.iter(|| {
            let req = FontisanRequest {
                header: FontisanHeader::current::<FontisanRequest>(),
                op: OP_ECHO,
                flags: 0,
                input: data.as_ptr(),
                input_len: data.len(),
            };
            let mut out = ptr::null_mut();
            let s = unsafe { fontisan_call(&req, &mut out) };
            unsafe { fontisan_result_free(black_box(out)) };
            black_box(s)
        })
    });
}

criterion_group!(benches, echo, corpus);
criterion_main!(benches);
