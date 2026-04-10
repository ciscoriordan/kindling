#!/bin/bash
set -e

# =============================================================================
# Test dictionary generator for debugging Kindle white screen issue
# Generates ~15 MOBI dictionaries, each isolating different variables
# =============================================================================

KINDLING_DIR="/Users/franciscoriordan/Documents/kindling"
SRC_MOBI="$KINDLING_DIR/src/mobi.rs"
SRC_KF8="$KINDLING_DIR/src/kf8.rs"
OPF_ORIG="/Users/franciscoriordan/Documents/lemma/lemma_greek_en_20260409_basic/lemma_greek_en_20260409_basic.opf"
OPF_DIR="$(dirname "$OPF_ORIG")"
OUTPUT_DIR="/Users/franciscoriordan/Documents/lemma/test_dicts"
BINARY="$KINDLING_DIR/target/release/kindling-cli"

mkdir -p "$OUTPUT_DIR"

# Track timing
SCRIPT_START=$(date +%s)

# Cleanup function to restore source on exit (including errors/interrupts)
cleanup() {
    echo ""
    echo "=== Restoring source code to HEAD ==="
    cd "$KINDLING_DIR"
    git checkout -- .
    echo "Source restored."
}
trap cleanup EXIT

# Function: create a modified OPF with unique title and identifier
# Usage: create_test_opf <test_id> <description>
# Returns: path to modified OPF via $TEST_OPF variable
create_test_opf() {
    local test_id="$1"
    local description="$2"
    local test_title="${test_id} ${description}"
    local test_identifier="${test_id}-$(date +%s)"

    TEST_OPF="$OPF_DIR/${test_id}.opf"
    sed \
        -e "s|<dc:title>.*</dc:title>|<dc:title>${test_title}</dc:title>|" \
        -e "s|<dc:identifier[^>]*>.*</dc:identifier>|<dc:identifier id=\"BookId\" opf:scheme=\"UUID\">${test_identifier}</dc:identifier>|" \
        "$OPF_ORIG" > "$TEST_OPF"
}

# Function: remove test OPF
remove_test_opf() {
    rm -f "$TEST_OPF"
}

# Function: restore source files to HEAD
restore_source() {
    cd "$KINDLING_DIR"
    git checkout -- .
}

# Function: build the binary
build_binary() {
    echo "  Building..."
    cd "$RUST_DIR"
    cargo build --release 2>&1 | tail -3
}

# Function: run the binary
# Usage: run_binary <output_mobi> [extra_flags...]
run_binary() {
    local output="$1"
    shift
    echo "  Running kindling-cli build..."
    "$BINARY" build "$TEST_OPF" -o "$output" "$@" 2>&1 | tail -10
}

# Function: run one full test
# Usage: run_test <test_id> <description> <patch_function> <flags...>
run_test() {
    local test_id="$1"
    local description="$2"
    local patch_fn="$3"
    shift 3
    local flags=("$@")

    local test_start=$(date +%s)
    echo ""
    echo "========================================================================"
    echo "=== ${test_id}: ${description}"
    echo "=== Flags: ${flags[*]:-none}"
    echo "========================================================================"

    # Restore source to clean state first
    restore_source

    # Apply patches
    echo "  Applying patches: ${patch_fn}"
    $patch_fn

    # Build
    build_binary

    # Create test OPF
    create_test_opf "$test_id" "$description"

    # Run
    local output_mobi="$OUTPUT_DIR/${test_id}_${description// /_}.mobi"
    run_binary "$output_mobi" "${flags[@]}"

    # Remove test OPF
    remove_test_opf

    local test_end=$(date +%s)
    local elapsed=$((test_end - test_start))
    local size=$(stat -f%z "$output_mobi" 2>/dev/null || echo "0")
    echo "  Done: ${output_mobi} ($(( size / 1024 / 1024 )) MB, ${elapsed}s)"
    echo ""
}

# =============================================================================
# Patch functions
# Each function modifies the source files for a specific test variant
# =============================================================================

patch_none() {
    # No changes - use current code as-is
    :
}

patch_revert_trailing_bytes() {
    # Revert trailing byte order from [0x81, 0x00] to [0x00, 0x81] in mobi.rs and kf8.rs
    # There are 3 locations in mobi.rs and 2 in kf8.rs where trailing bytes are pushed.
    # We need to swap the order: currently 0x81 first, then 0x00. Revert to 0x00 first, then 0x81.

    # mobi.rs: parallel compression (inside thread spawn)
    # The pattern is two consecutive lines with push(0x81) then push(0x00)
    # We use perl for multiline replacement
    perl -i -0pe 's/compressed\.push\(0x81\);\n(\s+)compressed\.push\(0x00\);/compressed.push(0x00);\n${1}compressed.push(0x81);/g' "$SRC_MOBI"
    perl -i -0pe 's/rec\.push\(0x81\);\n(\s+)rec\.push\(0x00\);/rec.push(0x00);\n${1}rec.push(0x81);/g' "$SRC_MOBI"

    # kf8.rs: same pattern
    perl -i -0pe 's/compressed\.push\(0x81\);\n(\s+)compressed\.push\(0x00\);/compressed.push(0x00);\n${1}compressed.push(0x81);/g' "$SRC_KF8"
    perl -i -0pe 's/rec\.push\(0x81\);\n(\s+)rec\.push\(0x00\);/rec.push(0x00);\n${1}rec.push(0x81);/g' "$SRC_KF8"
}

patch_remove_trailing_bytes() {
    # Remove trailing bytes entirely (no 0x81 or 0x00 appended)
    # Comment out the push lines

    # mobi.rs
    perl -i -0pe 's|(// Trailing bytes.*?\n\s+)compressed\.push\(0x81\);\n\s+compressed\.push\(0x00\);|${1}// REMOVED: trailing bytes|g' "$SRC_MOBI"
    perl -i -0pe 's|(// Trailing bytes.*?\n\s+)rec\.push\(0x81\);\n\s+rec\.push\(0x00\);|${1}// REMOVED: trailing bytes|g' "$SRC_MOBI"

    # kf8.rs
    perl -i -0pe 's|(// Trailing bytes.*?\n\s+)compressed\.push\(0x81\);\n\s+compressed\.push\(0x00\);|${1}// REMOVED: trailing bytes|g' "$SRC_KF8"
    perl -i -0pe 's|(// Trailing bytes.*?\n\s+)rec\.push\(0x81\);\n\s+rec\.push\(0x00\);|${1}// REMOVED: trailing bytes|g' "$SRC_KF8"
}

patch_revert_css() {
    # Revert CSS preservation in strip_idx_markup:
    # Replace the style extraction block with the old simple head replacement

    # The current code (lines ~1297-1317) extracts styles from <head>, builds new_head with styles,
    # then replaces. We revert to just replacing <head>...</head> with <head><guide></guide></head>

    perl -i -0pe '
s{    // Extract any <style>.*?</style> blocks from the <head> before replacing it\n    let style_re = Regex::new\(r"\(\?s\)<style\[^>\]\*>\..*?</style>"\)\.unwrap\(\);\n    let head_re = Regex::new\(r"\(\?s\)<head>\..*?</head>"\)\.unwrap\(\);\n    let style_block: String = head_re\n        \.find\(\&result\)\n        \.map\(\|head_match\| \{\n            style_re\n                \.find_iter\(head_match\.as_str\(\)\)\n                \.map\(\|m\| m\.as_str\(\)\.to_string\(\)\)\n                \.collect::<Vec<_>>\(\)\n                \.join\(""\)\n        \}\)\n        \.unwrap_or_default\(\);\n    let new_head = if style_block\.is_empty\(\) \{\n        "<head><guide></guide></head>"\.to_string\(\)\n    \} else \{\n        format!\("<head>\{\}<guide></guide></head>", style_block\)\n    \};\n    result = head_re\n        \.replace_all\(\&result, new_head\.as_str\(\)\)\n        \.to_string\(\);}{    // Simple head replacement (CSS extraction reverted)\n    let head_re = Regex::new(r"(?s)<head>.*?</head>").unwrap();\n    result = head_re\n        .replace_all(\&result, "<head><guide></guide></head>")\n        .to_string();}
' "$SRC_MOBI"
}

patch_revert_css_simple() {
    # Simpler approach: just sed the specific lines
    # Replace the style_block conditional with a simple replacement

    # Step 1: Replace the style extraction + conditional new_head with simple head replacement
    # We do this by replacing the entire block between the "Extract any" comment and the head_re.replace_all

    # First, replace the head extraction to be simple
    sed -i '' 's|let new_head = if style_block.is_empty() {|// Reverted: no CSS preservation|' "$SRC_MOBI"
    sed -i '' 's|"<head><guide></guide></head>".to_string()|let _unused_style = style_block;|' "$SRC_MOBI"
    sed -i '' 's|format!("<head>{}<guide></guide></head>", style_block)|// reverted|' "$SRC_MOBI"
    sed -i '' 's|result = head_re|result = head_re|' "$SRC_MOBI"
    sed -i '' 's|.replace_all(&result, new_head.as_str())|.replace_all(\&result, "<head><guide></guide></head>")|' "$SRC_MOBI"

    # This approach is too fragile. Let's use a different strategy:
    # Just use git show to get the old version of the function and do targeted replacement.
    # Actually, the simplest: replace the whole style_block / new_head block.
    :
}

patch_revert_css_v2() {
    # Strategy: use python for reliable multiline replacement
    python3 -c "
import re
with open('$SRC_MOBI', 'r') as f:
    content = f.read()

# Replace the CSS extraction block in strip_idx_markup
old = '''    // Extract any <style>...</style> blocks from the <head> before replacing it
    let style_re = Regex::new(r\"(?s)<style[^>]*>.*?</style>\").unwrap();
    let head_re = Regex::new(r\"(?s)<head>.*?</head>\").unwrap();
    let style_block: String = head_re
        .find(&result)
        .map(|head_match| {
            style_re
                .find_iter(head_match.as_str())
                .map(|m| m.as_str().to_string())
                .collect::<Vec<_>>()
                .join(\"\")
        })
        .unwrap_or_default();
    let new_head = if style_block.is_empty() {
        \"<head><guide></guide></head>\".to_string()
    } else {
        format!(\"<head>{}<guide></guide></head>\", style_block)
    };
    result = head_re
        .replace_all(&result, new_head.as_str())
        .to_string();'''

new = '''    // Simple head replacement (no CSS preservation)
    let head_re = Regex::new(r\"(?s)<head>.*?</head>\").unwrap();
    result = head_re
        .replace_all(&result, \"<head><guide></guide></head>\")
        .to_string();'''

content = content.replace(old, new)
with open('$SRC_MOBI', 'w') as f:
    f.write(content)
"
}

patch_revert_front_matter() {
    # Skip the front matter sections loop in build_text_content_by_letter
    # Comment out: for fm in &front_matter_sections { body_sections.push(fm.clone()); }
    python3 -c "
with open('$SRC_MOBI', 'r') as f:
    content = f.read()

old = '''    // Prepend front matter sections
    for fm in &front_matter_sections {
        body_sections.push(fm.clone());
    }'''

new = '''    // Prepend front matter sections (DISABLED for testing)
    // for fm in &front_matter_sections {
    //     body_sections.push(fm.clone());
    // }'''

content = content.replace(old, new)
with open('$SRC_MOBI', 'w') as f:
    f.write(content)
"
}

patch_revert_hr_separators() {
    # Change entry close replacement from <hr/> back to empty string
    sed -i '' 's|result = entry_close.replace_all(&result, "<hr/>").to_string();|result = entry_close.replace_all(\&result, "").to_string();|' "$SRC_MOBI"
}

patch_revert_style_in_byletter() {
    # In build_text_content_by_letter, disable the style block in combined head
    python3 -c "
with open('$SRC_MOBI', 'r') as f:
    content = f.read()

content = content.replace(
    'let style_block = first_style.unwrap_or_default();',
    'let style_block = String::new(); // style disabled for testing\n    let _ = first_style;'
)
with open('$SRC_MOBI', 'w') as f:
    f.write(content)
"
}

patch_revert_all() {
    # Revert ALL changes from both commits (bd4e70c and 5f2d5c5)
    # Get the mobi.rs and kf8.rs from BEFORE bd4e70c (i.e., bd4e70c^)
    cd "$KINDLING_DIR"
    git show bd4e70c^:src/mobi.rs > "$SRC_MOBI"
    git show bd4e70c^:src/kf8.rs > "$SRC_KF8"
}

patch_revert_bd4_only() {
    # Revert only the bd4e70c commit changes (CSS, front matter, separators)
    # but keep the trailing byte swap from 5f2d5c5
    # Strategy: get mobi.rs from after bd4e70c^ (before bd4e70c), then apply trailing byte swap
    cd "$KINDLING_DIR"
    git show bd4e70c^:src/mobi.rs > "$SRC_MOBI"
    # Now apply the trailing byte swap (change 0x00,0x81 to 0x81,0x00)
    perl -i -0pe 's/compressed\.push\(0x00\);\n(\s+)compressed\.push\(0x81\);/compressed.push(0x81);\n${1}compressed.push(0x00);/g' "$SRC_MOBI"
    perl -i -0pe 's/rec\.push\(0x00\);\n(\s+)rec\.push\(0x81\);/rec.push(0x81);\n${1}rec.push(0x00);/g' "$SRC_MOBI"
    # kf8.rs wasn't changed by bd4e70c, only by 5f2d5c5, so current kf8.rs is fine
}

patch_revert_all_keep_splitting() {
    # Revert everything from yesterday BUT keep the kindle-limits splitting logic from v0.6.0
    # The splitting logic (build_text_content_by_letter) was added in the kindle-limits commit (66c928f),
    # which is BEFORE yesterday's commits. So reverting both yesterday's commits keeps the splitting.
    patch_revert_all
}

# =============================================================================
# Combined patch functions
# =============================================================================

patch_revert_css_combined() {
    patch_revert_css_v2
    patch_revert_style_in_byletter
}

# =============================================================================
# Run all tests
# =============================================================================

echo "=================================================================="
echo "Test Dictionary Generator - Kindle White Screen Debug"
echo "=================================================================="
echo "Source OPF: $OPF_ORIG"
echo "Output dir: $OUTPUT_DIR"
echo "Started at: $(date)"
echo ""

# --- Group A: Trailing byte order ---

run_test "Test01" "Current" \
    patch_none \
    --kindle-limits

run_test "Test02" "NoKindleLimits" \
    patch_none \
    --no-kindle-limits

run_test "Test03" "OldTrailingBytes" \
    patch_revert_trailing_bytes \
    --kindle-limits

run_test "Test04" "OldTrailingBytes NoKL" \
    patch_revert_trailing_bytes \
    --no-kindle-limits

run_test "Test05" "NoTrailingBytes" \
    patch_remove_trailing_bytes \
    --kindle-limits

# --- Group B: CSS and style changes ---

run_test "Test06" "NoCSS" \
    patch_revert_css_combined \
    --kindle-limits

run_test "Test07" "NoCSS NoKL" \
    patch_revert_css_combined \
    --no-kindle-limits

# --- Group C: Front matter ---

run_test "Test08" "NoFrontMatter" \
    patch_revert_front_matter \
    --kindle-limits

# --- Group D: Entry separators ---

run_test "Test09" "NoHrSeparators" \
    patch_revert_hr_separators \
    --kindle-limits

# --- Group E: Compound reversions ---

run_test "Test10" "RevertAll KL" \
    patch_revert_all \
    --kindle-limits

run_test "Test11" "RevertAll NoKL" \
    patch_revert_all \
    --no-kindle-limits

run_test "Test12" "RevertBd4Only" \
    patch_revert_bd4_only \
    --kindle-limits

# --- Group F: Compression ---

run_test "Test13" "NoCompress" \
    patch_none \
    --no-compress --kindle-limits

run_test "Test14" "NoCompress NoKL" \
    patch_none \
    --no-compress --no-kindle-limits

# --- Group G: Minimal ---

run_test "Test15" "OldBytes NoCSS NoFM NoHR" \
    patch_revert_all_keep_splitting \
    --kindle-limits

# =============================================================================
# Summary
# =============================================================================

SCRIPT_END=$(date +%s)
TOTAL_ELAPSED=$((SCRIPT_END - SCRIPT_START))

echo ""
echo "=================================================================="
echo "ALL TESTS COMPLETE"
echo "=================================================================="
echo "Total time: ${TOTAL_ELAPSED}s ($(( TOTAL_ELAPSED / 60 ))m $(( TOTAL_ELAPSED % 60 ))s)"
echo "Output directory: $OUTPUT_DIR"
echo ""
echo "Generated files:"
ls -lh "$OUTPUT_DIR"/Test*.mobi 2>/dev/null || echo "  (no files found)"
echo ""
echo "Test matrix:"
echo "  Test01: Current code, --kindle-limits (BASELINE - what's broken)"
echo "  Test02: Current code, --no-kindle-limits"
echo "  Test03: Old trailing bytes [0x00,0x81], --kindle-limits"
echo "  Test04: Old trailing bytes [0x00,0x81], --no-kindle-limits"
echo "  Test05: No trailing bytes at all, --kindle-limits"
echo "  Test06: No CSS preservation, --kindle-limits"
echo "  Test07: No CSS preservation, --no-kindle-limits"
echo "  Test08: No front matter, --kindle-limits"
echo "  Test09: No <hr/> separators, --kindle-limits"
echo "  Test10: Revert ALL yesterday, --kindle-limits"
echo "  Test11: Revert ALL yesterday, --no-kindle-limits"
echo "  Test12: Revert bd4e70c only (CSS/FM/HR), --kindle-limits"
echo "  Test13: No compression, --kindle-limits"
echo "  Test14: No compression, --no-kindle-limits"
echo "  Test15: Revert all yesterday + keep splitting, --kindle-limits"
echo ""
echo "Source restored to HEAD."
