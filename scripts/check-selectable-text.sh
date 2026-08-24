#!/usr/bin/env bash
# check-selectable-text.sh — enforce the read-only-text selection rule.
#
# All copyable read-only text in the modal must go through the
# `gpui-selectable-text` crate. Constructing `gpui::StyledText` /
# `InteractiveText` in `crates/aura/src` bypasses its selection behavior, so
# it's a build failure.
#
# See docs/engineering/ui-selectable-text.md. Wired into scripts/pre-pr.sh.
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

SRC="crates/aura/src"

# Match `StyledText` or `InteractiveText` as whole identifiers, in any .rs file
# under $SRC. Comments/strings are close enough — application code should not
# name these implementation types now that the crate owns the element.
hits="$(
    grep -rnE '\b(StyledText|InteractiveText)\b' "$SRC" --include='*.rs' || true
)"

if [[ -n "$hits" ]]; then
    echo "Forbidden StyledText/InteractiveText under ${SRC}:"
    echo "$hits"
    echo
    echo "Render copyable read-only text via gpui_selectable_text::SelectableText instead."
    echo "See docs/engineering/ui-selectable-text.md."
    exit 1
fi

echo "ok — no raw StyledText/InteractiveText under ${SRC}"
