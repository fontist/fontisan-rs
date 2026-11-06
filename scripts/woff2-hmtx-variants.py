#!/usr/bin/env python3
"""Derive WOFF2 files with a transformed hmtx table (the real-world corpus has
none, and woff2_compress never emits one) from a plain single-font WOFF2.

  woff2-hmtx-variants.py <in.woff2> <reference.ttf> <out dir>

<reference.ttf> is woff2_decompress's output for <in.woff2> (used only for each
glyph's xMin). Needs `pip install brotli fonttools`. Writes
<stem>.hmtx<flags>.woff2 for every flag combination the font allows
(bit0: proportional lsbs dropped, bit1: monospace lsbs dropped).
"""
import io, struct, sys, os, brotli
from fontTools.ttLib import TTFont

K = ["cmap","head","hhea","hmtx","maxp","name","OS/2","post","cvt ","fpgm","glyf","loca","prep","CFF ","VORG","EBDT","EBLC","gasp","hdmx","kern","LTSH","PCLT","VDMX","vhea","vmtx","BASE","GDEF","GPOS","GSUB","EBSC","JSTF","MATH","CBDT","CBLC","COLR","CPAL","SVG ","sbix","acnt","avar","bdat","bloc","bsln","cvar","fdsc","feat","fmtx","fvar","gvar","hsty","just","lcar","mort","morx","opbd","prop","trak","Zapf","Silf","Glat","Gloc","Feat","Sill"]

def b128(d, p):
    v = 0
    for _ in range(5):
        c = d[p]; p += 1
        v = (v << 7) | (c & 0x7F)
        if not c & 0x80:
            return v, p
    raise ValueError

def enc128(v):
    out = [v & 0x7F]; v >>= 7
    while v:
        out.append(0x80 | (v & 0x7F)); v >>= 7
    return bytes(reversed(out))

src, ref, outdir = sys.argv[1:4]
d = open(src, "rb").read()
n = struct.unpack(">H", d[12:14])[0]
comp_len = struct.unpack(">I", d[20:24])[0]
assert struct.unpack(">IIIII", d[28:48]) == (0, 0, 0, 0, 0), "metadata/private blocks unsupported"
p = 48
ents = []
for _ in range(n):
    start = p
    fl = d[p]; p += 1
    idx = fl & 63
    tag = d[p:p+4].decode("latin1") if idx == 63 else K[idx]
    p += 4 if idx == 63 else 0
    ver = fl >> 6
    orig, p = b128(d, p)
    tr = (ver == 0) if tag in ("glyf", "loca") else ver != 0
    tlen = orig
    if tr:
        tlen, p = b128(d, p)
    ents.append(dict(tag=tag, start=start, end=p, orig=orig, tlen=tlen))
raw = brotli.decompress(d[p:p + comp_len])
off = 0
for e in ents:
    e["data"] = raw[off:off + e["tlen"]]; off += e["tlen"]
by = {e["tag"]: e for e in ents}
if "glyf" not in by or "hmtx" not in by:
    sys.exit("need glyf and hmtx")
assert (d[by["hmtx"]["start"]] >> 6) == 0, "hmtx already transformed"
ng = struct.unpack(">H", by["glyf"]["data"][4:6])[0]
nh = struct.unpack(">H", by["hhea"]["data"][34:36])[0]
f = TTFont(ref)
xmin = [getattr(f["glyf"][g], "xMin", 0) if f["glyf"][g].numberOfContours else 0 for g in f.getGlyphOrder()]
hm = by["hmtx"]["data"]
adv = [struct.unpack_from(">H", hm, 4 * i)[0] for i in range(nh)]
lsb = [struct.unpack_from(">h", hm, 4 * i + 2)[0] for i in range(nh)] + [struct.unpack_from(">h", hm, 4 * nh + 2 * (i - nh))[0] for i in range(nh, ng)]
prop_ok = all(lsb[i] == xmin[i] for i in range(nh))
mono_ok = all(lsb[i] == xmin[i] for i in range(nh, ng))
stem = os.path.basename(src)[:-len(".woff2")]
os.makedirs(outdir, exist_ok=True)
for flags in (1, 2, 3):
    if (flags & 1 and not prop_ok) or (flags & 2 and not mono_ok):
        continue
    body = struct.pack(">B", flags) + b"".join(struct.pack(">H", a) for a in adv)
    if not flags & 1:
        body += b"".join(struct.pack(">h", lsb[i]) for i in range(nh))
    if not flags & 2:
        body += b"".join(struct.pack(">h", lsb[i]) for i in range(nh, ng))
    new = []
    for e in ents:
        if e is by["hmtx"]:
            fl = (d[e["start"]] & 0x3F) | (1 << 6)
            new.append((bytes([fl]) + enc128(e["orig"]) + enc128(len(body)), body))
        else:
            new.append((d[e["start"]:e["end"]], e["data"]))
    directory = b"".join(x[0] for x in new)
    stream = brotli.compress(b"".join(x[1] for x in new), quality=5)
    pad = (-(48 + len(directory) + len(stream))) % 4
    total = 48 + len(directory) + len(stream) + pad
    hdr = bytearray(d[:48])
    hdr[8:12] = struct.pack(">I", total)
    hdr[20:24] = struct.pack(">I", len(stream))
    open(os.path.join(outdir, f"{stem}.hmtx{flags}.woff2"), "wb").write(bytes(hdr) + directory + stream + b"\0" * pad)
    print(f"{stem}.hmtx{flags}.woff2 (numGlyphs={ng}, numHMetrics={nh})")
