//! Memory model leak check with a counting allocator: runs on every platform
//! and toolchain. Single test in its own binary so no other thread allocates
//! concurrently. CI additionally runs the ffi suites under AddressSanitizer.
use fontisan::ffi::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::ptr;
use std::sync::atomic::{AtomicIsize, Ordering};

struct Counting;
static LIVE: AtomicIsize = AtomicIsize::new(0);
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        LIVE.fetch_add(1, Ordering::SeqCst);
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        LIVE.fetch_sub(1, Ordering::SeqCst);
        unsafe { System.dealloc(p, l) }
    }
}
#[global_allocator]
static A: Counting = Counting;

fn request(op: u32, input: &[u8]) -> FontisanRequest {
    FontisanRequest {
        header: FontisanHeader::current::<FontisanRequest>(),
        op,
        flags: 0,
        input: input.as_ptr(),
        input_len: input.len(),
    }
}

fn call(req: &FontisanRequest) -> (i32, *mut FontisanResult) {
    let mut out = ptr::null_mut();
    let status = unsafe { fontisan_call(req, &mut out) };
    (status, out)
}

#[test]
fn memory_model_is_leak_free() {
    // Warm up lazily-initialised std state, then require zero net allocations.
    let (_, out) = call(&request(OP_ECHO, b"warm"));
    unsafe { fontisan_result_free(out) };
    let before = LIVE.load(Ordering::SeqCst);
    for i in 0..1000u32 {
        let data = vec![i as u8; (i % 97) as usize];
        let mut req = request(OP_ECHO, &data);
        if i % 3 == 0 {
            req.header.abi_version = 0; // error path allocates a message
        }
        let (_, out) = call(&req);
        unsafe { fontisan_result_free(out) };
    }
    let after = LIVE.load(Ordering::SeqCst);
    // `data` above is freed by the loop; only engine blocks could remain.
    assert_eq!(
        after,
        before,
        "engine leaked {} allocations",
        after - before
    );
}
