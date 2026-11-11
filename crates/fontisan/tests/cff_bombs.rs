//! Hostile CFF1 tables built in-process (round 3, NOT RUN in round 1): deep subroutine
//! recursion, subroutine fan-out and INDEX bombs must fail or finish fast and bounded.
//! A counting allocator records the peak of live heap bytes.
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::Mutex;
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
/// The allocator counters are global: one test at a time.
static SERIAL: Mutex<()> = Mutex::new(());

fn peak_of<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let base = LIVE.load(Relaxed);
    PEAK.store(base, Relaxed);
    let r = f();
    (r, PEAK.load(Relaxed).saturating_sub(base))
}

fn num(v: i16) -> Vec<u8> {
    let mut o = vec![28];
    o.extend_from_slice(&v.to_be_bytes());
    o
}
fn dnum(v: i32) -> Vec<u8> {
    let mut o = vec![29];
    o.extend_from_slice(&v.to_be_bytes());
    o
}
fn index(items: &[Vec<u8>]) -> Vec<u8> {
    if items.is_empty() {
        return vec![0, 0];
    }
    let mut o = (items.len() as u16).to_be_bytes().to_vec();
    o.push(4);
    let mut at = 1u32;
    o.extend_from_slice(&at.to_be_bytes());
    for it in items {
        at += it.len() as u32;
        o.extend_from_slice(&at.to_be_bytes());
    }
    for it in items {
        o.extend_from_slice(it);
    }
    o
}

/// CFF1 with a Private DICT (so charstrings run), `local` as its Subrs.
fn cff(charstrings: &[Vec<u8>], global: &[Vec<u8>], local: &[Vec<u8>]) -> Vec<u8> {
    let hdr = vec![1, 0, 4, 1];
    let names = index(&[b"A".to_vec()]);
    let strings = index(&[]);
    let gs = index(global);
    let top = |cso: i32, off: i32| {
        let mut d = dnum(cso);
        d.push(17);
        d.extend(dnum(6));
        d.extend(dnum(off));
        d.push(18);
        index(&[d])
    };
    let cso = (hdr.len() + names.len() + top(0, 0).len() + strings.len() + gs.len()) as i32;
    let cs = index(charstrings);
    let off = cso + cs.len() as i32;
    let mut priv_dict = dnum(6);
    priv_dict.push(19);
    [
        hdr,
        names,
        top(cso, off),
        strings,
        gs,
        cs,
        priv_dict,
        index(local),
    ]
    .concat()
}

fn sfnt(tables: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
    let n = tables.len();
    let mut d = b"OTTO".to_vec();
    d.extend_from_slice(&(n as u16).to_be_bytes());
    d.extend_from_slice(&[0; 6]);
    let mut off = 12 + 16 * n;
    let mut body = Vec::new();
    for (tag, t) in tables {
        d.extend_from_slice(*tag);
        d.extend_from_slice(&0u32.to_be_bytes());
        d.extend_from_slice(&(off as u32).to_be_bytes());
        d.extend_from_slice(&(t.len() as u32).to_be_bytes());
        body.extend_from_slice(t);
        while body.len() % 4 != 0 {
            body.push(0);
        }
        off = 12 + 16 * n + body.len();
    }
    d.extend(body);
    d
}

fn font(num_glyphs: u16, charstrings: &[Vec<u8>], global: &[Vec<u8>]) -> Vec<u8> {
    let mut head = vec![0u8; 54];
    head[18..20].copy_from_slice(&1000u16.to_be_bytes());
    let mut hhea = vec![0u8; 36];
    hhea[34..36].copy_from_slice(&1u16.to_be_bytes());
    let mut maxp = 0x0000_5000u32.to_be_bytes().to_vec();
    maxp.extend_from_slice(&num_glyphs.to_be_bytes());
    sfnt(&[
        (b"CFF ", cff(charstrings, global, &[vec![11]])),
        (b"head", head),
        (b"hhea", hhea),
        (b"maxp", maxp),
    ])
}

/// `n` glyphs that each call global subr 0 (bias 107 below 1240 subrs).
fn calls_subr0(n: usize) -> Vec<Vec<u8>> {
    let mut g = num(-107);
    g.extend([29, 14]);
    vec![g; n]
}

/// Each glyph draws `segs` line segments through a shared subr: a small file whose
/// per-glyph outline is large.
fn segment_font(n: usize, segs: usize) -> Vec<u8> {
    let mut sub = Vec::new();
    for _ in 0..segs {
        sub.extend(num(1));
        sub.extend(num(1));
        sub.push(5); // rlineto
    }
    sub.push(11);
    let mut mv = num(0);
    mv.extend(num(0));
    mv.push(21); // rmoveto
    let mut cs = mv;
    cs.extend(num(-107));
    cs.extend([29, 14]);
    font(n as u16, &vec![cs; n], &[sub])
}

#[test]
fn subr_self_recursion_fails_fast_in_every_consumer() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut sub = num(-107);
    sub.push(29);
    let f = font(2000, &calls_subr0(2000), &[sub]);
    let t = std::time::Instant::now();
    let c = fontisan::convert::convert(&f, fontisan::convert::Target::Ttf, 0)
        .expect_err("recursion is an error");
    assert!(c.message.contains("too deep"), "{}", c.message);
    let s = fontisan::subset::subset(&f, &[1], "web").expect_err("recursion is an error");
    assert!(s.message.contains("too deep"), "{}", s.message);
    assert!(t.elapsed().as_secs() < 5);
}

#[test]
fn subset_does_not_retain_the_outlines_of_dropped_glyphs() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    // 400 glyphs x 20000 segments: ~450 MiB of outlines if every glyph is kept.
    let f = segment_font(400, 20_000);
    let (r, peak) = peak_of(|| fontisan::subset::subset(&f, &[1], "web"));
    r.expect("subset");
    assert!(peak < 64 << 20, "peak {} MiB", peak >> 20);
}

/// `n` glyphs that each run about a million no-op operators through one shared subr.
fn nop_font(n: usize) -> Vec<u8> {
    let mut sub = vec![15u8; 1_000_000];
    sub.push(11);
    font(n as u16, &calls_subr0(n), &[sub])
}

#[test]
fn total_charstring_work_is_bounded_per_font() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let f = nop_font(200); // 200 M operators if unbounded
    let t = std::time::Instant::now();
    let e = fontisan::convert::convert(&f, fontisan::convert::Target::Ttf, 0)
        .expect_err("a font-wide budget stops it");
    assert!(e.message.contains("operation budget"), "{}", e.message);
    let e = fontisan::subset::subset(&f, &[1], "web").expect_err("and the subsetter");
    assert!(e.message.contains("operation budget"), "{}", e.message);
    assert!(t.elapsed().as_secs() < 60, "took {:?}", t.elapsed());
}

#[test]
fn a_font_inside_the_budget_still_converts_glyph_by_glyph() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let f = nop_font(20);
    // Past the charstrings the rebuilt font lacks `post`; that error is not the budget.
    let e = fontisan::convert::convert(&f, fontisan::convert::Target::Ttf, 0).unwrap_err();
    assert!(!e.message.contains("operation budget"), "{}", e.message);
}
