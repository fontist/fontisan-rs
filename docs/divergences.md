# Divergences found after the goldens were cut

Two cases the Phase 0 goldens never exercised. Rust now reproduces current Ruby
(`fontisan-phase0` head) exactly; tests are in `crates/fontisan/tests/divergences.rs`
and pin the measured output.

## 1. WOFF1 `read` reports the requested mode

- Ruby: `Fontisan::Reference.read(woff, mode: :full)[0]["mode"]` is `"full"`;
  with `mode: :metadata` it is `"metadata"`. Everything else in the document is
  identical between the two modes.
- Old Rust always wrote `"metadata"` for WOFF1 (inferred from the read goldens,
  which dedupe variants with `same_as` and so never compared `mode` for WOFF1).
- Fix: `read_face` echoes the requested mode for every format. Table gating is unchanged.
- Measured with: `bundle exec ruby -Ilib -e 'require "fontisan"; %i[full metadata].each { |m| p Fontisan::Reference.read("corpus/fonts/source-sans-3-regular-otf-woff/SourceSans3-Regular.otf.woff", mode: m)[0]["mode"] }'`

## 2. `audit(validate: true)` on a table with a bad checksum (known reference bug)

- Ruby root cause: `lib/fontisan/audit/checks/table_directory_check.rb`, in
  `validate_checksums`, the mismatch message calls `entry.checksum.to_s(16)`.
  `entry.checksum` is a `BinData::Uint32be`, whose `to_s` takes no argument, so it
  raises `ArgumentError: wrong number of arguments (given 1, expected 0)`.
  The check runner turns that into one fatal issue and the check yields no other issue.
- Ruby report for `corpus/derived/malformed-bad-checksum.ttf`:
  `{"category":"table_directory","message":"Check table_directory failed to execute: ArgumentError: wrong number of arguments (given 1, expected 0)","severity":"fatal"}`,
  summary `error_count 0, fatal_count 1, info_count 1, warning_count 1, passed false`.
- Old Rust: a `checksum mismatch` error issue for `glyf`.
- Fix: Rust returns the same `ArgumentError` from the check at the first non-`head`
  table whose checksum mismatches.
- Measured with: `bundle exec ruby -Ilib -e 'require "fontisan"; require "json"; puts JSON.generate(Fontisan::Reference.audit("corpus/derived/malformed-bad-checksum.ttf", validate: true))'`

## Pathological GSUB/GPOS tables past the work budget return `[]` for the remaining scripts

- Ruby resolves every script's features independently, so a table whose scripts each
  point at thousands of LangSys records costs scripts x table length. The fuzz seed
  `fuzz/seeds/timeout-font-layout-scripts.ttf` (165 KB parsed as a table body: 18,176
  scripts, about 65.8M LangSys records reachable) takes Ruby minutes and took the Rust
  port 61 s before the fix.
- Rust: `LayoutTable::script_features()` caps the LangSys work (records plus newly read
  words) at 2 units per table byte. Past the cap the remaining scripts read as a failed
  read, `[]`, where Ruby would eventually resolve them. `features(script)` for a single
  script is unbudgeted (it is bounded by the table itself).
- A real font cannot reach the cutoff: `layout_equivalence` asserts every corpus font
  stays under 25% of the budget.

## Follow-up (fontisan repo, not done here)

Add both cases to the Phase 0 goldens: a read golden for a WOFF1 font with a
non-`same_as` `mode` entry, and a `known_reference_bug` audit golden for
`malformed-bad-checksum.ttf` with `validate: true`.
