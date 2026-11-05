# frozen_string_literal: true

# 1b substrate spike: regenerates crates/fontisan/tests/spike/expected.json
# from the Ruby reference. Goldens did not exist when the spike ran (card 0b),
# so the expected values come straight from `Fontisan::Reference` plus the
# Ruby gvar/glyf parsers. Run from a fontisan checkout (Ruby 3.4):
#
#   cd <fontisan> && bundle exec ruby -Ilib <fontisan-rs>/scripts/spike-expected.rb \
#       <corpus dir containing fonts/ and derived/> <out.json>
#
# Per-glyph "lines" are hashed (SHA-256 over lines joined by "\n") so the
# committed fixture stays small; spike.rs builds the same lines from read-fonts.
require "json"
require "digest"
require "fontisan"

corpus, out = ARGV
sha = ->(s) { Digest::SHA256.hexdigest(s) }
tup = ->(t) { t.map { |f| (f * 16_384).round }.join(",") }

result = { "reference" => `git rev-parse HEAD`.strip, "fonts" => {} }

Dir[File.join(corpus, "{fonts,derived}", "*")].sort.each do |path|
  base = File.basename(path)
  next unless base.end_with?(".ttf", ".otf") && !base.start_with?("malformed")

  entry = {}
  face = Fontisan::Reference.read(path, mode: :full, lazy: false).first
  entry["cmap"] = face["cmap"]
  entry["outlines"] = face["outlines"]

  font = Fontisan::FontLoader.load(path, mode: :full, lazy: false)
  if font.has_table?("glyf") && font.has_table?("loca")
    glyf = font.table("glyf")
    loca = font.table("loca")
    head = font.table("head")
    maxp = font.table("maxp")
    loca.parse_with_context(head.index_to_loc_format, maxp.num_glyphs) unless loca.parsed?
    lines = Array.new(maxp.num_glyphs) do |gid|
      g = glyf.glyph_for(gid, loca, head)
      if g.nil? then "e"
      elsif g.simple?
        ends = g.end_pts_of_contours.map(&:to_i)
        ["s", g.num_contours.to_i, g.num_points.to_i, *g.bounding_box.map(&:to_i), ends.join(" "),
         sha.call(g.x_coordinates.pack("l<*")), sha.call(g.y_coordinates.pack("l<*")),
         sha.call(g.flags.map { |f| f & 1 }.pack("C*"))].join(",")
      else
        "c,#{g.component_glyph_ids.map(&:to_i).join(' ')}"
      end
    end
    kinds = lines.group_by { |l| l[0] }.transform_values(&:size)
    entry["glyf_parsed"] = { "num_glyphs" => lines.size, "kinds" => kinds, "sha256" => sha.call(lines.join("\n")) }
  end

  if font.has_table?("gvar")
    g = Fontisan::Tables::Gvar.read(font.table_data["gvar"])
    shared = g.shared_tuples.map { |t| t.map { |f| (f * 16_384).round } }
    lines = Array.new(g.glyph_count.to_i) do |gid|
      d = g.glyph_variation_data(gid)
      next "-" if d.nil?

      t = g.glyph_tuple_variations(gid)
      tuples = t[:tuples].map do |u|
        peak = u[:embedded_peak] ? u[:peak].map { |f| (f * 16_384).round } : shared[u[:shared_index].to_i]
        [peak.join(","), u[:intermediate] ? tup.call(u[:start]) : "", u[:intermediate] ? tup.call(u[:end]) : ""].join("|")
      end
      [sha.call(d), d.bytesize, t[:tuple_count], tuples.join(";")].join(":")
    end
    entry["gvar"] = {
      "version" => [g.major_version.to_i, g.minor_version.to_i], "axis_count" => g.axis_count.to_i,
      "shared_tuple_count" => g.shared_tuple_count.to_i, "glyph_count" => g.glyph_count.to_i,
      "flags" => g.flags.to_i, "shared_tuples_sha256" => sha.call(shared.map { |t| t.join(",") }.join("\n")),
      "glyphs_with_data" => lines.count { |l| l != "-" }, "glyphs_sha256" => sha.call(lines.join("\n"))
    }
  end
  result["fonts"][base] = entry
end

# Malformed kinds: how Ruby behaves at load time and at each later table access.
# Per kind: "load" => "ok" or the exception class; "data" => tag => bytes
# actually read (nil when the offset is past EOF, short when truncated);
# "parse" => tag => "ok" or the exception class from `font.table(tag)`.
result["malformed"] = {}
Dir[File.join(corpus, "derived", "malformed-*")].sort.each do |path|
  rec = {}
  begin
    font = Fontisan::FontLoader.load(path, mode: :full, lazy: false)
    rec["load"] = "ok"
    rec["data"] = font.table_data.to_h { |t, d| [t, d&.bytesize] }.sort.to_h
    rec["parse"] = %w[head hhea maxp OS/2 post cmap name].to_h do |t|
      font.table(t)
      [t, "ok"]
    rescue Exception => e # rubocop:disable Lint/RescueException
      [t, e.class.name]
    end
  rescue Exception => e # rubocop:disable Lint/RescueException
    rec["load"] = e.class.name
  end
  result["malformed"][File.basename(path)] = rec
end

File.write(out, "#{JSON.pretty_generate(result)}\n")
