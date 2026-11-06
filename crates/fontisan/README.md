# fontisan

Pure-Rust font engine: read, scan, audit and convert fonts (TTF, OTF, TTC,
WOFF, WOFF2). It is the engine behind the [fontisan](https://github.com/fontist/fontisan)
Ruby gem and has no C dependencies. An optional, versioned C ABI is available
behind the `ffi` feature (header: `include/fontisan.h`).

```toml
[dependencies]
fontisan = "0.1"
```

```rust
// Scanning never panics on bad input: a failure becomes a row with an error.
let rows = fontisan::scan::scan_bytes(b"not a font", "broken.ttf");
assert_eq!(rows.len(), 1);
```

Minimum supported Rust version: 1.85. License: BSD-3-Clause.
