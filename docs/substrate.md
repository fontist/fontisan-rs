# Substrate evaluation: read-fonts 0.46 / skrifa 0.49 (card 1b)

Spike only. `read-fonts =0.46.0` and `skrifa =0.49.0` are dev-dependencies of
`fontisan`; no production adapter uses them yet (1c-1f build those).

Expected values: goldens did not exist yet (card 0b), so they come from the Ruby
reference at fontisan `0ea515c` via `scripts/spike-expected.rb`, committed as
`crates/fontisan/tests/spike/expected.json` (cmap and outline digests from
`Fontisan::Reference.read(mode: :full, lazy: false)`; parsed glyf, gvar and the
malformed behaviours from the Ruby parsers). Tests read fonts from the corpus
fetched by `cargo xtask corpus-fetch` and skip with a message when it is absent
(`FONTISAN_SPIKE_REQUIRE_CORPUS=1` makes absence a failure). Measured on the 23
ci-tier ttf/otf fonts (plus derived quirk/names fonts) and the 8 malformed kinds.

## Licence, MSRV, C-free

- Licence: read-fonts, skrifa, font-types are `MIT OR Apache-2.0`; transitive
  crates are MIT/Apache/Zlib/Unicode-3.0, all in `deny.toml`'s allow list.
  `cargo deny` is not installed here: NOT RUN.
- MSRV: read-fonts 0.46.0, skrifa 0.49.0, font-types 0.12.7 declare
  `rust-version = 1.85`, equal to ours. `cargo +1.85.0 test` and clippy pass.
- No C: graph of read-fonts+skrifa (normal+build) has 18 packages, none `*-sys`,
  none with `links` (measured with `cargo metadata`). `scripts/check-no-c.sh`
  only walks normal+build of the shipped crates, so it does not see these
  dev-only deps; it passes (2 packages). When 1c makes them normal deps it
  will cover them.

## Findings (what the adapter must add)

1. **Eager directory check.** `FontRef::new` accepts `malformed-bad-num-tables`
   and `malformed-truncated-100` (records run past EOF); Ruby rejects both at
   load. Adapter guard: `12 + 16*numTables <= len` (no valid corpus font trips it).
2. **Truncated table body.** Ruby hands back the short bytes of a table that
   runs past EOF and fails when parsing it (`name` in truncated-half: EOFError).
   read-fonts returns no data for that table, so `name()` errors at the same
   access point. Same failure point, different error class/message: the adapter
   maps errors to Ruby's class names; table-data length for such a table differs
   (Ruby 32 bytes, read-fonts none).
3. **Zero-length cmap** (`quirk-empty-cmap-table`): `cmap()` errors, Ruby returns
   an empty result (count 0). Adapter maps "present, zero bytes" to empty.
4. **cmap selection** is Ruby's own: one subtable, (3,10) then (3,1) then first
   platform-0, only formats 4 and 12, gid 0 dropped, `subtable_formats` over all
   records. Implemented on read-fonts records it matches. skrifa `Charmap`
   matched counts on this corpus but uses its own selection and more formats, so
   it is not a guarantee; do not use it for the cmap golden.
5. Bad per-table / head checksums are never verified by read-fonts, like Ruby.
6. Offsets past EOF (bad-table-offset): read-fonts gives no data for that table
   (Ruby nil); other tables parse.

## Verdicts

| Table (card) | Verdict | Basis |
|---|---|---|
| cmap (1c) | USE read-fonts, with Ruby selection rule and empty-table mapping | TESTED: count, codepoint digest, formats equal on 23 fonts |
| glyf/loca (1d) | USE read-fonts | TESTED: per-glyph byte-range digest equals Ruby `outlines.digest` on 19 fonts; parsed simple glyphs (contours, points, bbox, coordinates, on-curve) and composite component ids equal Ruby on all glyphs |
| gvar (1d) | USE read-fonts | TESTED: header, flags, shared tuples, per-glyph raw data digest, tuple count, peak and intermediate regions equal Ruby on 4 variable ttf. Point-delta parity not yet golden-backed |
| skrifa outlines at variable locations (1d/1h) | USE skrifa for drawing/instancing, parity unproven | TESTED only that all glyphs draw and gvar moves points; no Ruby instance golden exists yet (float vs Ruby op order is a 1h risk) |
| table directory, TTC (1c) | USE read-fonts + directory guard | TESTED for ttf/otf directory and malformed; TTC REASONED (`FontRef::from_index`, `CollectionRef`) |
| head, hhea, maxp, OS/2, post (1c) | USE read-fonts | REASONED, plus malformed parse points TESTED (ok/fail matches Ruby). Field-by-field parity vs `header_fields` not tested |
| name (1c) | USE read-fonts, hand-decode strings | REASONED: records exposed; Ruby's `decode_name_record_manually` rules must be re-implemented; truncation point TESTED |
| CFF, CFF2 (1d) | USE read-fonts for INDEX/CharString bytes; hash bytes ourselves | REASONED: `cff.rs`/`cff2.rs` present; Ruby hashes CharString bytes |
| fvar, STAT, HVAR, MVAR (1d) | USE read-fonts | REASONED: `fvar`, `stat`, `hvar`, `mvar` present; digests need raw Fixed integers, available via `to_bits` |
| avar, cvar (1d) | USE read-fonts | REASONED |
| WOFF / WOFF2 decode (1e) | HAND-ROLL on miniz_oxide / brotli | REASONED: read-fonts has no WOFF/WOFF2 container; (skrifa/read-fonts do not decode) |
| GSUB/GPOS/GDEF enumeration (1f) | USE read-fonts | REASONED: `gsub`, `gpos`, `layout` present; script/feature lists are direct |
| audit checks, indexability (1f) | HAND-ROLL on top of read-fonts tables | REASONED: Ruby-specific logic |
| Collections, dfont, Type 1 | OUT of read-fonts scope | dfont/Type 1 routed to Ruby by plan |

## Premises checked

- read-fonts can match Ruby for cmap, glyf, gvar: CONFIRMED (with findings 1-3).
- Lazy failure points reproducible: CONFIRMED for bad offset, bad checksum,
  truncated-half; PARTLY FALSE out of the box: read-fonts rejects nothing about
  numTables overrun (findings 1), so an explicit guard is needed.
- MSRV <= 1.85: CONFIRMED (exactly 1.85).
