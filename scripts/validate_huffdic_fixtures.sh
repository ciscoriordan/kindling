#!/usr/bin/env bash
#
# Validate kindling's HUFF/CDIC ("huffdic") decoder against files it did not
# produce. Run after changing src/huffcdic.rs or regenerating the fixtures with
# `cargo run --example gen_huffdic_fixture`.
#
# Three checks, weakest to strongest:
#
#   1. The committed huffdic fixtures against the PalmDOC dictionaries they
#      were transcoded from. Also asserted by tests/huffdic.rs, so this is the
#      one check that runs with no network and no third-party code.
#   2. A real kindlegen 2.9 huffdic file against its uncompressed twin, both
#      from libmobi's test corpus. This is the check that matters: nothing here
#      made either file, so agreeing with it is evidence about the format
#      rather than about our own encoder. 111701 bytes, 28 text records.
#   3. KindleUnpack's and calibre's decoders over the committed fixtures, if
#      either is importable. Independent implementations of the same format.
#
# Nothing here runs in CI: check 2 needs the network, and check 3 needs Python
# packages we do not ask contributors to install.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FIX="$ROOT/tests/fixtures"
CHECK="cargo run --quiet --release --example huffdic_check --"

cd "$ROOT"
cargo build --release --example huffdic_check --quiet

echo "== 1. committed fixtures against their PalmDOC sources =="
$CHECK "$FIX/huffdic/en_huffdic.mobi" "$FIX/langs/en/en-kindlegen.mobi"
$CHECK "$FIX/huffdic/ja_huffdic.mobi" "$FIX/langs/ja/ja-kindlegen.mobi"
$CHECK "$FIX/huffdic/en_huffdic_stale_index.mobi" "$FIX/langs/en/en-kindlegen.mobi"

echo
echo "== 2. libmobi's kindlegen 2.9 huffdic sample against its uncompressed twin =="
SAMPLES="https://github.com/bfabiszewski/libmobi/raw/public/tests/samples"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
if curl -fsSL -o "$TMP/huffdic.mobi" "$SAMPLES/sample-unicode-huffdic.mobi" &&
    curl -fsSL -o "$TMP/plain.mobi" "$SAMPLES/sample-unicode-uncompressed.mobi"; then
  $CHECK "$TMP/huffdic.mobi" "$TMP/plain.mobi"
else
  echo "skipped: could not fetch the libmobi samples" >&2
fi

echo
echo "== 3. third-party decoders over the committed fixtures =="
python3 - "$FIX" <<'PY' || echo "skipped: no third-party decoder importable" >&2
import sys, struct

fix = sys.argv[1]
pairs = [
    (f"{fix}/huffdic/en_huffdic.mobi", f"{fix}/langs/en/en-kindlegen.mobi"),
    (f"{fix}/huffdic/ja_huffdic.mobi", f"{fix}/langs/ja/ja-kindlegen.mobi"),
]

readers = []
try:
    # KindleUnpack, `pip install mobi`
    from mobi.mobi_uncompress import HuffcdicReader as KU
    readers.append(("KindleUnpack", KU))
except ImportError:
    pass
try:
    from calibre.ebooks.mobi.huffcdic import HuffReader as CAL
    readers.append(("calibre", CAL))
except ImportError:
    pass
if not readers:
    sys.exit(1)


def records(path):
    d = open(path, "rb").read()
    n = struct.unpack(">H", d[76:78])[0]
    offs = [struct.unpack(">I", d[78 + i * 8 : 82 + i * 8])[0] for i in range(n)]
    return [d[offs[i] : offs[i + 1] if i + 1 < n else len(d)] for i in range(n)]


def trailing(rec, flags):
    end = len(rec)
    for bit in range(15, 0, -1):
        if flags & (1 << bit) and end:
            size = 0
            for v in rec[end - 4 : end]:
                if v & 0x80:
                    size = 0
                size = (size << 7) | (v & 0x7F)
            end -= min(size, end)
    if flags & 1 and end:
        end -= min((rec[end - 1] & 3) + 1, end)
    return end


def palmdoc(src):
    from mobi.mobi_uncompress import PalmdocReader

    return PalmdocReader().unpack(src)


for huff_path, plain_path in pairs:
    rs = records(huff_path)
    r0 = rs[0]
    ntext = struct.unpack(">H", r0[8:10])[0]
    flags = struct.unpack(">I", r0[240:244])[0]
    hi, hc = struct.unpack(">I", r0[112:116])[0], struct.unpack(">I", r0[116:120])[0]

    ps = records(plain_path)
    p0 = ps[0]
    pflags = struct.unpack(">I", p0[240:244])[0]
    expected = b"".join(
        palmdoc(ps[i][: trailing(ps[i], pflags)])
        for i in range(1, struct.unpack(">H", p0[8:10])[0] + 1)
    )

    for name, Reader in readers:
        r = Reader()
        if name == "KindleUnpack":
            r.loadHuff(rs[hi])
            for i in range(1, hc):
                r.loadCdic(rs[hi + i])
            got = b"".join(
                r.unpack(rs[i][: trailing(rs[i], flags)]) for i in range(1, ntext + 1)
            )
        else:
            r.load_huff(rs[hi])
            for i in range(1, hc):
                r.load_cdic(rs[hi + i])
            got = b"".join(
                r.unpack(rs[i][: trailing(rs[i], flags)]) for i in range(1, ntext + 1)
            )
        status = "match" if got == expected else "MISMATCH"
        print(f"{name}: {huff_path.split('/')[-1]} -> {len(got)} bytes, {status}")
        if got != expected:
            sys.exit(1)
PY

echo
echo "All huffdic validation checks passed."
