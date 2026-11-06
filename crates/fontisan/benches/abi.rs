//! Criterion skeleton. 3c reports native numbers from here next to the Ruby
//! ratio; real read/scan/convert benches are added by their cards.
use criterion::{Criterion, criterion_group, criterion_main};
use fontisan::ffi::*;
use std::hint::black_box;
use std::ptr;

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

criterion_group!(benches, echo);
criterion_main!(benches);
