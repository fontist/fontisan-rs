# frozen_string_literal: true

# 1c: regenerates crates/fontisan/tests/read/expected.json from the Ruby
# reference facade (Fontisan::Reference.read). Used until card 0b publishes the
# real read goldens under corpus/goldens/read (TODO: switch the harness to them
# and delete this script and the fixture). Run from a fontisan checkout (Ruby 3.4):
#
#   cd <fontisan> && bundle exec ruby -Ilib -rfontisan <fontisan-rs>/scripts/read-expected.rb \
#       <corpus dir containing fonts/ and derived/> <out.json>
#
# Scope is the 1c surface: tables (directory, lengths, sha256, parse error),
# names, header_fields (head/hhea/maxp/OS/2), cmap. fvar and outlines belong to
# 1d. WOFF/WOFF2 (1e), dfont and Type 1 (routed to Ruby) are not read here.
require "json"
require "digest"

corpus, out = ARGV
KEEP = %w[format file_sha256 tables header_fields cmap].freeze
COMBOS = [[:full, false], [:full, true], [:metadata, false], [:metadata, true]].freeze

result = { "reference" => `git rev-parse HEAD`.strip, "fonts" => {} }

Dir[File.join(corpus, "{fonts,derived}", "*")].sort.each do |path|
  base = File.basename(path)
  next unless base.end_with?(".ttf", ".otf", ".ttc", ".otc")
  next if base.start_with?("malformed")

  by_combo = COMBOS.to_h do |mode, lazy|
    [ "#{mode}/lazy=#{lazy}", Fontisan::Reference.read(path, mode: mode, lazy: lazy) ]
  end
  base_faces = by_combo["full/lazy=false"]
  faces = base_faces.each_index.map do |i|
    names = base_faces[i]["names"]
    by_combo.each_value { |fs| raise "names differ across combos in #{base}" unless fs[i]["names"] == names }
    post = Fontisan::FontLoader.load(path, font_index: i, mode: :full, lazy: false).table("post")
    glyph_names = post&.glyph_names
    { "names" => names,
      "post_glyph_names" => glyph_names && { "count" => glyph_names.size,
                                             "sha256" => Digest::SHA256.hexdigest(glyph_names.join("\n")) },
      "combos" => by_combo.transform_values { |fs| fs[i].slice(*KEEP) } }
  end
  result["fonts"][base] = { "faces" => faces }
end

File.write(out, "#{JSON.generate(result)}\n")
