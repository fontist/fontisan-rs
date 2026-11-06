# frozen_string_literal: true

# 1c: how the Ruby reference reacts to a table body cut to N bytes. Writes
# crates/fontisan/tests/read/truncation.json. The Rust test rebuilds the same
# fonts (sequential 4-byte-aligned layout) from the same corpus fonts.
#   cd <fontisan> && bundle exec ruby -Ilib -rfontisan <fontisan-rs>/scripts/truncation-expected.rb <fonts dir> <out.json>
require "json"
require "tmpdir"

TAGS = %w[head hhea maxp OS/2 post name cmap].freeze
LENGTHS = [0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 12, 14, 16, 20, 24, 28, 30, 31, 32, 33, 34, 35, 36, 38, 40, 44,
           50, 53, 54, 55, 60, 68, 70, 76, 78, 80, 86, 88, 92, 95, 96, 97, 100].freeze
SOURCES = %w[TestTTF.ttf TestOTF.otf].freeze

def build(tables)
  tags = tables.keys.sort
  n = tags.size
  off = 12 + 16 * n
  dir = +""
  body = +""
  tags.each do |t|
    d = tables[t]
    dir << [t].pack("a4") << [0, off + body.bytesize, d.bytesize].pack("NNN")
    body << d << ("\0" * ((4 - d.bytesize % 4) % 4))
  end
  [0x00010000, n, 0, 0, 0].pack("Nnnnn") + dir + body
end

fonts_dir, out = ARGV
result = {}
Dir.mktmpdir do |tmp|
  SOURCES.each do |src|
    font = Fontisan::FontLoader.load(File.join(fonts_dir, src), mode: :full, lazy: false)
    base = font.table_names.to_h { |t| [t, font.table_data[t]] }
    res = {}
    TAGS.each do |tag|
      next unless base[tag]

      full = base[tag].bytesize
      lens = (LENGTHS.select { |l| l <= full } + [full]).uniq
      res[tag] = { "full_length" => full }
      lens.each do |len|
        path = File.join(tmp, "t.ttf")
        File.binwrite(path, build(base.merge(tag => base[tag].byteslice(0, len))))
        outcome =
          begin
            f = Fontisan::FontLoader.load(path, mode: :full, lazy: false)
            t = f.table(tag)
            t.name_records.each { |r| r.string_offset } if tag == "name"
            t.unicode_mappings if tag == "cmap"
            "ok"
          rescue Exception => e # rubocop:disable Lint/RescueException
            e.class.name
          end
        res[tag][len.to_s] = outcome
      end
    end
    result[src] = res
  end
end
File.write(out, "#{JSON.pretty_generate(result)}\n")
