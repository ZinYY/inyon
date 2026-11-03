#!/usr/bin/env bash
# Checks the skills under skills/ the way someone installing them would use them. Each needs a
# SKILL.md with a name that matches its folder and a description. For blueprint, it starts a site
# from the template, renders every sample screen at phone and desktop sizes (failing on a console
# error or a button that does nothing), and drives progress.py through each command.
# Needs Node and Playwright's Chromium (BLUEPRINT_PLAYWRIGHT names a folder that has Playwright).
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

for dir in "$root"/skills/*/; do
  name=$(basename "$dir")
  head -1 "$dir/SKILL.md" | grep -qx -- '---' || { echo "$name: SKILL.md has no frontmatter"; exit 1; }
  grep -qx "name: $name" "$dir/SKILL.md" || { echo "$name: SKILL.md's name isn't $name"; exit 1; }
  grep -q '^description: ..' "$dir/SKILL.md" || { echo "$name: SKILL.md has no description"; exit 1; }
done

bp="$root/skills/blueprint"
for f in "$bp"/scripts/*.mjs "$bp"/assets/site/kit/*.js "$bp"/assets/site/*.js "$bp"/assets/site/mockups/*.js; do node --check "$f"; done
python3 -m py_compile "$bp/scripts/progress.py"

node "$bp/scripts/new-site.mjs" "$tmp/site" --name "Test app"
grep -q "name: 'Test app'" "$tmp/site/kit/data.js"
if node "$bp/scripts/new-site.mjs" "$tmp/site" --name x 2>/dev/null; then echo "new-site.mjs copied over a site that wasn't empty"; exit 1; fi

p="python3 $bp/scripts/progress.py --file $tmp/site/progress.json"
$p task r1-sync done "synced"
$p add R2 r2-extra "One more task"
$p link r2-extra https://example.com/pr/1
$p now "Checking"
$p log "A line"
python3 - "$tmp/site/progress.json" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
r1, r2 = d['releases'][0], d['releases'][1]
assert [t['status'] for t in r1['tasks'] if t['id'] == 'r1-sync'] == ['done'], r1
assert any(t['id'] == 'r2-extra' and t['link'] == 'https://example.com/pr/1' for t in r2['tasks']), r2
assert d['now'] == 'Checking' and d['log'][0]['text'] == 'A line', d['log'][:2]
PY
if $p task nope done 2>/dev/null; then echo "progress.py took an unknown task"; exit 1; fi

node "$bp/scripts/stills.mjs" "$tmp/site" --audit

# a plan with no screens (--no-mockups): nothing links to them, and the plan and the matrix still
# render with no console errors or dead buttons
node "$bp/scripts/new-site.mjs" "$tmp/bare" --name "Parts" --no-mockups
for f in mockups kit/shell.js kit/menus.js; do [ ! -e "$tmp/bare/$f" ] || { echo "--no-mockups left $f"; exit 1; }; done
if grep -q 'mockups/' "$tmp/bare/index.html"; then echo "--no-mockups left a link to the screens"; exit 1; fi
node "$bp/scripts/serve.mjs" "$tmp/bare" --port 0 > "$tmp/serve.log" &
spid=$!
for _ in $(seq 50); do grep -q 'http://' "$tmp/serve.log" && break; sleep 0.1; done
url=$(grep -o 'http://[^ ]*' "$tmp/serve.log") || { echo "serve.mjs didn't start"; kill $spid; exit 1; }
for page in index.html matrix.html; do
  for size in "390 844" "1440 900"; do
    said=$(AUDIT=1 node "$bp/scripts/shoot.mjs" "$url$page" "$tmp/bare-shot.png" $size light)
    if ! grep -q 'no dead controls' <<<"$said" || ! grep -q 'no console errors' <<<"$said"; then echo "$page at $size:"; echo "$said"; kill $spid; exit 1; fi
  done
done
kill $spid

# publishing to Inyon's list of pages, serve.mjs against the hub's /v/ fixtures, progress.py's locking
bun test "$bp/tests"
python3 "$bp/tests/test_progress.py"
echo "skills ok"
