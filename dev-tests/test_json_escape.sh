#!/bin/bash
# =============================================================================
# JSON escaping test for start_macos.command
#
# Validates that paths containing Chinese characters, spaces, and double quotes
# are correctly JSON-escaped in the generated settings.local.json.
#
# Dependencies: bash, Ruby (macOS built-in, /usr/bin/ruby) — NO Python/Node/jq.
# Location: knowledge-companion/dev-tests/ — development only, not runtime.
#
# Usage: bash dev-tests/test_json_escape.sh
# =============================================================================

set -euo pipefail

# ── Same json_escape as start_macos.command ─────────────────────────────────
json_escape() {
    local s="${1-}"
    s="${s//\\/\\\\}"
    s="${s//\"/\\\"}"
    printf '%s' "$s"
}

PASS=0
FAIL=0

# ── Ruby helper script for JSON validation ──────────────────────────────────
# We write it to a temp file so the bundle path (with special chars) doesn't
# need to be injected into Ruby -e command line.
RUBY_SCRIPT=$(mktemp /tmp/ks-validate-XXXXXX.rb)
cleanup_ruby_script() { rm -f "$RUBY_SCRIPT"; }
trap cleanup_ruby_script EXIT

# ── Unit test: json_escape function ─────────────────────────────────────────
test_escape_unit() {
    local desc="$1"
    local input="$2"
    local expected="$3"
    local result
    result=$(json_escape "$input")

    if [[ "$result" == "$expected" ]]; then
        echo "  OK   [escape/$desc]"
        ((PASS++)) || true
    else
        echo "  FAIL [escape/$desc]"
        echo "        input:    '$input'"
        echo "        expected: '$expected'"
        echo "        got:      '$result'"
        ((FAIL++)) || true
    fi
}

echo "=== Unit tests: json_escape function ==="
test_escape_unit "plain ASCII"           "/Volumes/USB/KnowledgeSuite"  "/Volumes/USB/KnowledgeSuite"
test_escape_unit "Chinese"               "/tmp/测试知识套件"             "/tmp/测试知识套件"
test_escape_unit "spaces"                "/tmp/my test bundle"          "/tmp/my test bundle"
test_escape_unit "double quote"          '/path/has"quote"'             '/path/has\"quote\"'
test_escape_unit "backslash"             '/path/has\backslash'          '/path/has\\backslash'
test_escape_unit "backslash and quote"   '/path\and"both"'             '/path\\and\"both\"'
test_escape_unit "multiple quotes"       '/a"b"c"'                     '/a\"b\"c\"'

# ── Integration test: generate JSON with special paths ──────────────────────
test_json_generation() {
    local desc="$1"
    local bundle_path="$2"

    echo ""
    echo "=== Integration: $desc ==="
    echo "    Path: $bundle_path"

    # Create test bundle at the given path
    rm -rf "$bundle_path" 2>/dev/null || true
    mkdir -p "$bundle_path"/{scripts,workspace/.claw,data/{logs,cache},knowledge,config,bin}
    touch "$bundle_path/bin/knowledge-companion"
    touch "$bundle_path/bin/claw"
    cat > "$bundle_path/config/knowledge-companion.toml" << 'TOML'
[app]
name = "KnowledgeCompanion"
[knowledge]
root = "./knowledge"
[storage]
db_path = "./data/knowledge.db"
cache_dir = "./data/cache"
log_dir = "./data/logs"
TOML

    # Simulate start_macos.command config generation
    local BUNDLE_ROOT="$bundle_path"
    local ROOT_CMD ROOT_ENV ROOT_CFG
    ROOT_CMD="$(json_escape "$BUNDLE_ROOT/bin/knowledge-companion")"
    ROOT_ENV="$(json_escape "$BUNDLE_ROOT")"
    ROOT_CFG="$(json_escape "$BUNDLE_ROOT/config/knowledge-companion.toml")"

    local json_out="$bundle_path/workspace/.claw/settings.local.json"
    cat > "$json_out" << KCEOF
{
  "mcpServers": {
    "knowledge-companion": {
      "command": "$ROOT_CMD",
      "args": [],
      "env": {
        "KC_BUNDLE_ROOT": "$ROOT_ENV",
        "KC_CONFIG_PATH": "$ROOT_CFG"
      },
      "toolCallTimeoutMs": 120000
    }
  }
}
KCEOF

    echo "    Generated JSON:"
    sed 's/^/      /' "$json_out"

    # Write a Ruby script that validates the JSON file.
    # We use a temp file approach so special chars in the path don't
    # interfere with inline Ruby -e string escaping.
    cat > "$RUBY_SCRIPT" << RUBYEOF
require 'json'

json_path = File.read(File.join(File.dirname(__FILE__), 'path.txt')).strip
data = JSON.parse(File.read(json_path))

cmd  = data.dig('mcpServers', 'knowledge-companion', 'command')
root = data.dig('mcpServers', 'knowledge-companion', 'env', 'KC_BUNDLE_ROOT')
cfg  = data.dig('mcpServers', 'knowledge-companion', 'env', 'KC_CONFIG_PATH')

expected_cmd  = File.read(File.join(File.dirname(__FILE__), 'expected_cmd.txt')).strip
expected_root = File.read(File.join(File.dirname(__FILE__), 'expected_root.txt')).strip
expected_cfg  = File.read(File.join(File.dirname(__FILE__), 'expected_cfg.txt')).strip

errors = []
errors << "command: got #{cmd.inspect}, expected #{expected_cmd.inspect}"   if cmd  != expected_cmd
errors << "KC_BUNDLE_ROOT: got #{root.inspect}, expected #{expected_root.inspect}" if root != expected_root
errors << "KC_CONFIG_PATH: got #{cfg.inspect}, expected #{expected_cfg.inspect}"   if cfg  != expected_cfg

if errors.empty?
  puts 'ALL_MATCH'
else
  puts "MISMATCH: #{errors.join('; ')}"
  exit 1
end
RUBYEOF

    # Write expected values to temp files (avoids shell escaping issues)
    local tmpdir
    tmpdir=$(dirname "$RUBY_SCRIPT")
    echo "$json_out" > "$tmpdir/path.txt"
    echo "$BUNDLE_ROOT/bin/knowledge-companion" > "$tmpdir/expected_cmd.txt"
    echo "$BUNDLE_ROOT" > "$tmpdir/expected_root.txt"
    echo "$BUNDLE_ROOT/config/knowledge-companion.toml" > "$tmpdir/expected_cfg.txt"

    local result
    result=$(ruby "$RUBY_SCRIPT" 2>&1) || true

    if [[ "$result" == "ALL_MATCH" ]]; then
        echo "  OK   [$desc]: JSON valid, all fields round-trip correctly"
        ((PASS++)) || true
    else
        echo "  FAIL [$desc]: $result"
        ((FAIL++)) || true
    fi

    rm -rf "$bundle_path"
}

# ── Run integration tests ───────────────────────────────────────────────────
test_json_generation "normal ASCII"     "/tmp/ks-test-normal-$$"
test_json_generation "spaces in path"   "/tmp/my test bundle $$"
test_json_generation "Chinese 中文"      "/tmp/测试知识套件$$"
test_json_generation "Chinese + spaces" "/tmp/知识 套件 $$"
test_json_generation "mixed"            "/tmp/my 测试 bundle $$"

# Double-quote test (filesystem-dependent)
dquote_dir="/tmp/ks-quote\"test$$"
if mkdir -p "$dquote_dir" 2>/dev/null; then
    test_json_generation "double quote in path" "$dquote_dir"
else
    echo ""
    echo "=== Integration: double quote in path (skipped — FS restriction) ==="
    echo "    (Covered by unit test 'double quote' above)"
fi

# ── Summary ─────────────────────────────────────────────────────────────────
echo ""
echo "=============================================="
echo "  JSON escape tests: $PASS passed, $FAIL failed"
echo "=============================================="

exit $FAIL
