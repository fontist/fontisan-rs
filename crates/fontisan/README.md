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

## Error kinds

Every engine error reports the Ruby class and message (the Ruby tier is the contract) and a
stable `ErrorKind` (`InvalidFont`, `InvalidAxis`, `InvalidCoordinates`, `Io`, `Unsupported`,
`Subsetting`, `CorruptedTable`, `MissingTable`, `InvalidArgument`, `Truncated`, `Other`).
Map errors by kind, not by message: `sfnt::Error::kind()`, `convert::ConvertError::kind()`,
`subset::SubsetError::kind()`. Over the C ABI the kind is `fontisan_result.error_kind`
(`FONTISAN_KIND_*`). The field was appended to the result block without an ABI bump: the block
is engine-allocated and read-only, so callers built against the earlier 64-byte layout are
unaffected; `header.struct_size >= 72` (64-bit) says the field is present. Values are append
only.

Minimum supported Rust version: 1.85. License: BSD-3-Clause.
