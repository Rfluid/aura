#!/usr/bin/env bash
# check-selectable-text.sh — enforce the read-only-text selection rule.
#
# All copyable read-only text in the modal must go through
# `crates/aura/src/selectable_text.rs` (the `sel` / `sel_styled` / `sel_linked`
# helpers). Constructing `gpui::StyledText` / `InteractiveText` anywhere else in
# `crates/aura/src` bypasses drag-select + copy, so it's a build failure.
#
# See docs/engineering/ui-selectable-text.md. Wired into scripts/pre-pr.sh.
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

SRC="crates/aura/src"
ALLOWED="selectable_text.rs"

# Match `StyledText` or `InteractiveText` as whole identifiers, in any .rs file
# under $SRC except the sanctioned wrapper. Comments/strings are close enough —
# neither should name these types outside the wrapper anyway.
hits="$(
    grep -rnE '\b(StyledText|InteractiveText)\b' "$SRC" --include='*.rs' \
        | grep -v "/${ALLOWED}:" \
        || true
)"

if [[ -n "$hits" ]]; then
    echo "Forbidden StyledText/InteractiveText outside ${SRC}/${ALLOWED}:"
    echo "$hits"
    echo
    echo "Render copyable read-only text via crate::selectable_text::sel(..) instead."
    echo "See docs/engineering/ui-selectable-text.md."
    exit 1
fi

echo "ok — no raw StyledText/InteractiveText outside ${ALLOWED}"
