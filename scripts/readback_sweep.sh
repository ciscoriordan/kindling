#!/usr/bin/env bash
#
# Build every fixture and read each result back with a tool that is not
# kindling.
#
# The whole test suite can pass on a file no reader can open. That is not
# hypothetical here: the huffdic encoder round-tripped through kindling's own
# decoder, and calibre's decoder read the same bare streams perfectly, while
# calibre's conversion pipeline lost more than half the text. The difference
# was that the pipeline strips each record's trailing entries first, the way a
# device does, and the records were missing them. Nothing kindling owns could
# have found that, because kindling agreed with itself.
#
# So this is deliberately an OUTSIDE check. It asserts nothing about bytes; it
# asks whether another implementation can read what we wrote, which is the one
# question our own tests cannot ask.
#
# Two formats, two readers. calibre reads the MOBI/AZW3 side; sdcv reads the
# StarDict side, and it has to be sdcv rather than a sequential reader,
# because a StarDict index sorted the wrong way leaves headwords in the file
# that only a binary search can fail to reach. Both our own tests and
# pyglossary read those indexes sequentially and accepted a broken one
# (issue #60).
#
# On macOS calibre lives inside the app bundle, which is where this looks by
# default; override with EBOOK_CONVERT. sdcv comes from `brew install sdcv`
# and is skipped if absent.
#
#   ./scripts/readback_sweep.sh [output-dir]
#
# Exits non-zero if any built file fails to read back.

set -uo pipefail

K=${KINDLING:-./target/release/kindling-cli}
EC=${EBOOK_CONVERT:-/Applications/calibre.app/Contents/MacOS/ebook-convert}
OUT=${1:-$(mktemp -d)}

[ -x "$K" ] || { echo "No kindling-cli at $K. cargo build --release first." >&2; exit 1; }
[ -x "$EC" ] || { echo "No ebook-convert at $EC. Set EBOOK_CONVERT." >&2; exit 1; }

mkdir -p "$OUT"
echo "Building into $OUT"

# Fixtures with no text content at all. Reading one back as zero bytes of text
# is the correct answer, not a finding: this one is a single page holding one
# <img> and nothing else.
NO_TEXT="fixed_layout_missing_opf_fixed_layout_missing_opf"

built=0
failed=0
for opf in $(find tests/fixtures -name '*.opf' | sort); do
    name=$(echo "$opf" | sed 's|tests/fixtures/||; s|/|_|g; s|\.opf$||')
    for fmt in mobi azw3; do
        extra=""
        [ "$fmt" = mobi ] && extra="--legacy-mobi"
        target="$OUT/$name.$fmt"
        # A fixture that is meant to fail validation or fail to build is not
        # what this is looking for, so a build failure is skipped rather than
        # reported. --no-validate keeps the error fixtures building.
        if ! $K build "$opf" -o "$target" --no-validate $extra \
             >"$OUT/$name.$fmt.build.log" 2>&1; then
            continue
        fi
        built=$((built + 1))

        txt="$OUT/$name.$fmt.txt"
        if ! "$EC" "$target" "$txt" >"$OUT/$name.$fmt.calibre.log" 2>&1; then
            echo "  FAILED TO READ: $name.$fmt (see $OUT/$name.$fmt.calibre.log)"
            failed=$((failed + 1))
            continue
        fi
        case " $NO_TEXT " in
            *" $name "*) continue ;;
        esac
        size=$(wc -c < "$txt" | tr -d ' ')
        if [ "$size" -lt 40 ]; then
            echo "  READ BACK EMPTY: $name.$fmt is $size bytes of text"
            failed=$((failed + 1))
        fi
    done
done

# StarDict is a separate output format with separate readers, and its own
# failure mode: an index sorted the wrong way leaves headwords in the file
# that a binary search cannot reach, which every sequential reader (ours,
# pyglossary) happily accepts. sdcv is the one that settles it.
SDCV=${SDCV:-sdcv}
if command -v "$SDCV" >/dev/null 2>&1; then
    echo
    echo "StarDict bundles, read with sdcv:"
    for opf in $(grep -rl "idx:entry" tests/fixtures --include='*.html' 2>/dev/null \
                 | xargs -n1 dirname | sort -u \
                 | xargs -I{} sh -c 'ls {}/*.opf 2>/dev/null | head -1'); do
        name=$(echo "$opf" | sed 's|tests/fixtures/||; s|/|_|g; s|\.opf$||')
        bundle="$OUT/sd_$name"
        rm -rf "$bundle"
        $K stardict "$opf" -o "$bundle" >"$OUT/sd_$name.log" 2>&1 || continue
        # sdcv wants a directory of dictionary directories.
        root="$OUT/sdroot_$name"; rm -rf "$root"; mkdir -p "$root/d"
        cp "$bundle"/* "$root/d/" 2>/dev/null || continue
        ifo=$(ls "$root/d"/*.ifo 2>/dev/null | head -1)
        [ -n "$ifo" ] || continue
        words=$(python3 - "$root/d" <<'PY'
import glob, sys
p = glob.glob(sys.argv[1] + "/*.idx")[0]
d = open(p, "rb").read()
out, i = [], 0
while i < len(d) and len(out) < 12:
    j = d.index(b"\x00", i); out.append(d[i:j].decode("utf-8", "replace")); i = j + 9
print("\n".join(out))
PY
)
        bad=0; n=0
        while IFS= read -r w; do
            [ -n "$w" ] || continue
            n=$((n + 1))
            sdcv --data-dir "$root" -n -e "$w" 2>/dev/null | grep -qF "$w" || {
                echo "  UNREACHABLE: $name has '$w' in its index but sdcv cannot find it"
                bad=$((bad + 1)); failed=$((failed + 1))
            }
        done <<EOF2
$words
EOF2
        [ "$bad" -eq 0 ] && echo "  $name: $n headwords all reachable"
    done
else
    echo
    echo "sdcv not found, skipping the StarDict half (brew install sdcv)"
fi

echo
echo "built $built file(s), $failed could not be read back"
[ "$failed" -eq 0 ] || exit 1
