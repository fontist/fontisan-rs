//! WOFF1 convert holds the decoded tables once, not the decoded sfnt plus a copy of every
//! table (round 2 item 3: audit and convert peaked at about 3x the declared size).
//! A counting allocator records the peak of live heap bytes during the call.
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

struct Counting;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            PEAK.fetch_max(LIVE.fetch_add(l.size(), Relaxed) + l.size(), Relaxed);
        }
        p
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(l) };
        if !p.is_null() {
            PEAK.fetch_max(LIVE.fetch_add(l.size(), Relaxed) + l.size(), Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        LIVE.fetch_sub(l.size(), Relaxed);
        unsafe { System.dealloc(p, l) };
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, l, n) };
        if !q.is_null() {
            LIVE.fetch_sub(l.size(), Relaxed);
            PEAK.fetch_max(LIVE.fetch_add(n, Relaxed) + n, Relaxed);
        }
        q
    }
}

#[global_allocator]
static A: Counting = Counting;

const BIG: usize = 64 << 20;

/// `head` (stored) and `prep` (BIG zero bytes, deflated): no outlines, so convert stops
/// right after reading the tables.
fn woff1_bomb() -> Vec<u8> {
    let z = miniz_oxide::deflate::compress_to_vec_zlib(&vec![0u8; BIG], 6);
    let head = [0u8; 54];
    let dir = 44 + 2 * 20;
    let mut d = Vec::new();
    d.extend_from_slice(b"wOFF");
    d.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    d.extend_from_slice(&((dir + 56 + z.len()) as u32).to_be_bytes());
    d.extend_from_slice(&2u16.to_be_bytes());
    d.resize(44, 0);
    for (tag, off, comp, orig) in [(b"head", dir, 54, 54), (b"prep", dir + 56, z.len(), BIG)] {
        d.extend_from_slice(tag);
        for v in [off, comp, orig, 0] {
            d.extend_from_slice(&(v as u32).to_be_bytes());
        }
    }
    d.extend_from_slice(&head);
    d.extend_from_slice(&[0, 0]);
    d.extend_from_slice(&z);
    d
}

#[test]
fn woff1_convert_peak_stays_near_the_declared_size() {
    let file = woff1_bomb();
    LIVE.store(0, Relaxed);
    PEAK.store(0, Relaxed);
    let r = fontisan::convert::convert(&file, fontisan::convert::Target::Ttf, 0);
    let peak = PEAK.load(Relaxed);
    let e = r.expect_err("outline-less source must be rejected");
    assert!(
        e.message.contains("Cannot detect font format"),
        "{}",
        e.message
    );
    assert!(
        peak < BIG + BIG / 4,
        "peak {} MiB for a {} MiB table",
        peak >> 20,
        BIG >> 20
    );
}
