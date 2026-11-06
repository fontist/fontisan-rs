# frozen_string_literal: true

# Regenerates the CBDT/CBLC subset fixtures: for each case, the input font
# (built with the Ruby gem's spec/support/cbdt_fixture.rb, CBDT/CBLC replaced
# where a case needs strikes the stock fixture lacks) and Ruby's subset output
# (Reference.subset, web profile) as the expected bytes.
#
#   FONTISAN_ENGINE=ruby bundle exec ruby gen_fixtures.rb <ruby-gem-checkout> <out-dir>
#
# `fontisan_rs`/`Reference.subset` is the Ruby engine; the checkout is the
# ruby reference (read-only).
require "digest"
require "json"
require "tmpdir"

root = File.expand_path(ARGV.fetch(0))
out = File.expand_path(ARGV.fetch(1))
$LOAD_PATH.unshift(File.join(root, "lib"), File.join(root, "spec"))
ENV["FONTISAN_ENGINE"] = "ruby"
require "fontisan"
require "fontisan/reference"
require "support/cbdt_fixture"

CF = Fontisan::SpecHelpers::CbdtFixture

# ---- helpers to hand-build CBLC/CBDT -------------------------------------
def line_metrics
  [1, 0, 1, 0, 1, 0, 0, 0, 0, 0, 0, 0].pack("c*") # 12 one-byte fields
end

# strikes: [{ppem:, first:, last:, subs: [{first:, last:, fmt:, img:, off:, tail:}]}]
# tail = bytes following the 8-byte IndexSubTable header.
def build_cblc(strikes)
  out = [0x00030000, strikes.size].pack("NN")
  cursor = 8 + (strikes.size * 48)
  bodies = []
  strikes.each do |s|
    ista_off = cursor
    ista_len = s[:subs].size * 8
    ist_cursor = ista_off + ista_len
    ista = +"".b
    ists = +"".b
    s[:subs].each do |sub|
      ist = [sub[:fmt], sub[:img], sub[:off]].pack("nnN") + sub[:tail]
      ista << [sub[:first], sub[:last], ist_cursor - ista_off].pack("nnN")
      ists << ist
      ist_cursor += ist.bytesize
    end
    out << [ista_off, ista.bytesize + ists.bytesize, s[:subs].size, 0].pack("NNNN")
    out << line_metrics << line_metrics
    out << [s[:first], s[:last], s[:ppem], s[:ppem], 32, 2].pack("nnCCCc")
    bodies << (ista + ists)
    cursor = ist_cursor
  end
  (out + bodies.join).b
end

def block(tag, len)
  (tag.to_s * len)[0, len].b
end

def write_font(codepoints, cbdt, cblc, path)
  tables = CF.build_tables(codepoints)
  # Ruby's Builder needs an outline table for the composite closure: give
  # every glyph an empty simple glyph (numberOfContours 0, 12 bytes with a zero instruction length).
  n = tables["maxp"].unpack1("x4n")
  tables["glyf"] = ([0, 0, 0, 0, 0, 0].pack("n6") * n).b
  tables["loca"] = Array.new(n + 1) { |i| i * 6 }.pack("n*") # short offsets
  # the stock OS/2 is shorter than the v4 layout Ruby's parser reads
  tables["OS/2"] = tables["OS/2"].ljust(96, "\0".b)
  tables["CBDT"] = cbdt
  tables["CBLC"] = cblc
  tables.delete("CBLC") if cblc.nil?
  tables.delete("CBDT") if cbdt.nil?
  Fontisan::FontWriter.write_to_file(tables, path, sfnt_version: CF.sfnt_version)
end

# multi-strike case: 12 colour glyphs, gids 1..12
def multi_tables
  header = [3, 0].pack("nn")
  data = +header.b
  mark = lambda do |blk|
    off = data.bytesize
    data << blk
    off
  end
  # strike 16: fmt 1 for gids 1-3 (sizes 5,6,7), fmt 3 for gids 6-8 (size 8 each)
  s1 = [5, 6, 7].each_with_index.map { |n, i| block("a#{i}", n) }
  o1 = mark.call(s1.join)
  offs1 = [0, 5, 11, 18]
  s2 = Array.new(3) { |i| block("b#{i}", 8) }
  o2 = mark.call(s2.join)
  offs2 = [0, 2, 4, 6]
  # strike 32: fmt 2 for gids 5-9, constant size 12 each
  s3 = Array.new(5) { |i| block("c#{i}", 12) }
  o3 = mark.call(s3.join)
  # strike 48: fmt 1 for gids 11-12 (dropped when 11/12 not kept)
  s4 = [9, 10].each_with_index.map { |n, i| block("d#{i}", n) }
  o4 = mark.call(s4.join)
  strikes = [
    { ppem: 16, first: 1, last: 8, subs: [
      { first: 1, last: 3, fmt: 1, img: 17, off: o1, tail: offs1.pack("N*") },
      { first: 6, last: 8, fmt: 3, img: 18, off: o2, tail: offs2.pack("n*") },
    ] },
    { ppem: 32, first: 5, last: 9, subs: [
      { first: 5, last: 9, fmt: 2, img: 19, off: o3,
        tail: [12].pack("N") + ("\x01" * 8).b },
    ] },
    { ppem: 48, first: 11, last: 12, subs: [
      { first: 11, last: 12, fmt: 1, img: 17, off: o4, tail: [0, 9, 19].pack("N*") },
    ] },
  ]
  [data, build_cblc(strikes)]
end

# format 5 (sparse, ruby reads its offset array from the glyph id array bytes)
def fmt5_tables
  data = [3, 0].pack("nn").b
  o = data.bytesize
  data << block("e", 40)
  tail = [6].pack("N") + ("\x02" * 8).b + [3].pack("N") + [2, 3, 4].pack("n*")
  strikes = [
    { ppem: 24, first: 2, last: 4, subs: [
      { first: 2, last: 4, fmt: 5, img: 20, off: o, tail: tail },
    ] },
    { ppem: 12, first: 1, last: 3, subs: [
      { first: 1, last: 3, fmt: 1, img: 17, off: o, tail: [0, 3, 7, 12].pack("N*") },
    ] },
  ]
  [data, build_cblc(strikes)]
end

cases = []
Dir.mkdir(out) unless Dir.exist?(out)

add = lambda do |name, cps, cbdt, cblc, text|
  input = File.join(out, "#{name}.ttf")
  write_font(cps, cbdt, cblc, input)
  gids = Fontisan::Reference.glyph_ids(input, text: text)
  bytes = Fontisan::Reference.subset(input, text: text, profile: "web")
  File.binwrite(File.join(out, "#{name}.expected.ttf"), bytes)
  cases << { "name" => name, "text" => text, "gids" => gids,
             "input_sha256" => Digest::SHA256.file(input).hexdigest,
             "expected_sha256" => Digest::SHA256.hexdigest(bytes) }
end

stock = (0x41..0x46).to_a
add.call("stock_keep_some", stock, CF.cbdt_table(stock), CF.cblc_table(stock), "ACE")
add.call("stock_keep_all", stock, CF.cbdt_table(stock), CF.cblc_table(stock), "ABCDEF")

multi = (0x41..0x4C).to_a
d, l = multi_tables
# gids kept: 2,3,5,7,8,9 (B,C,E,G,H,I); strike 48 loses every glyph
add.call("multi_strike", multi, d, l, "BCEGHI")
add.call("multi_strike_all", multi, d, l, "ABCDEFGHIJKL")

f5d, f5l = fmt5_tables
add.call("format5", (0x41..0x44).to_a, f5d, f5l, "BCD")

File.write(File.join(out, "manifest.json"), "#{JSON.pretty_generate(cases)}\n")
puts "wrote #{cases.size} cases to #{out}"
