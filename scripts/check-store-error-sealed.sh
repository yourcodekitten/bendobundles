#!/usr/bin/env bash
# StoreError::Aws must carry ONLY the bounded AwsFault — never free text, and never a
# conversion from it.
#
# WHY A GUARD AND NOT A COMMENT: the whole design rests on this. `StoreError::Aws` used to
# hold `format!("{e:?}")`, an unbounded capture of an SDK `Debug` we do not own, flowing to
# the operator Discord channel. Deleting the blanket `From` impl is what forces every call
# site through the bounded extractor. **If a free-text arm or a text conversion is ever added
# back, the type becomes a sealer nothing forces — the exact defect it was built to remove —
# and every call site can quietly opt out again.** That regression is one line and would not
# look like a regression in review.
#
# ⚠️ ANCHORED TO THE DECLARATION, NOT TO PROSE. An earlier version of this check grepped for
# the banned form anywhere in the file and flagged the doc comment that explains the ban.
# A grep cannot tell a declaration from a sentence about one — so match the variant line.
#
# rc: 0 = sealed, 1 = a forbidden form is present, 2 = NOT MEASURED (fail closed).
set -uo pipefail

SRC="${1:-crates/dynamo/src/lib.rs}"
[ -r "$SRC" ] || { echo "NOT MEASURED — cannot read $SRC"; exit 2; }

# The variant must exist at all, or we are grepping a file that no longer declares it and a
# clean bill would be vacuous.
decls=$(grep -cE '^[[:space:]]+Aws\(' "$SRC")
[ "$decls" -eq 1 ] || {
  echo "NOT MEASURED — expected exactly 1 'Aws(' variant declaration in $SRC, found $decls"
  exit 2; }

payload=$(grep -oE '^[[:space:]]+Aws\([A-Za-z_]+\)' "$SRC" | grep -oE '\([A-Za-z_]+\)' | tr -d '()')
if [ "$payload" != "AwsFault" ]; then
  echo "🔴 StoreError::Aws carries '$payload', not AwsFault."
  echo "   A free-text payload makes the sealer optional — see this script's header."
  exit 1
fi

# And no conversion INTO the fault from free text, which would be the same hole one level down.
if grep -qE 'impl[[:space:]]+From<(String|&str|&'"'"'static str)>[[:space:]]+for[[:space:]]+AwsFault' \
     "$SRC" crates/dynamo/src/aws_fault.rs 2>/dev/null; then
  echo "🔴 a text conversion into AwsFault exists — that reopens the hole the type closes."
  exit 1
fi

echo "✅ StoreError::Aws(AwsFault) — sealed; no free-text payload, no text conversion"
