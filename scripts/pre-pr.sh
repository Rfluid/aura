#!/usr/bin/env bash
# pre-pr.sh — fix + check everything locally before pushing a PR.
# Mirrors: quality-and-security.yml, secret_scanning.yml (gitleaks).
# CodeQL (GitHub-only) and SBOM (artifact-only) are intentionally skipped.
#
# Actions vs checks:
#   rustfmt     → AUTO-FIXES with cargo fmt --all
#   everything else → check only
#
# Dependency graph (matches CI jobs):
#   Wave 1 (parallel): rustfmt | clippy | test | gitleaks
#   Wave 2:            cargo_audit   <- needs rustfmt + clippy + test
#   Wave 3:            build_release <- needs cargo_audit
#   gitleaks runs freely and never blocks other waves.
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'
CYAN='\033[0;36m'; BOLD='\033[1m'; DIM='\033[2m'; RESET='\033[0m'

WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT

declare -A PIDS STATUS
declare -a ORDER PENDING

banner() { echo -e "\n${CYAN}${BOLD}── $* ──${RESET}"; }
has_tool() { command -v "$1" &>/dev/null; }

declare -A HINTS
HINTS[rustfmt]="cargo fmt --all was run automatically — review and stage any resulting changes"
HINTS[clippy]="fix the reported lint failures; run: cargo clippy --workspace --all-targets --locked -- -D warnings"
HINTS[selectable_text]="route copyable text through gpui_selectable_text::SelectableText; run: bash scripts/check-selectable-text.sh"
HINTS[test]="fix the failing tests; run: cargo test --workspace --locked"
HINTS[cargo_audit]="upgrade the vulnerable dependency or ignore an accepted advisory explicitly"
HINTS[gitleaks]="remove the secret from history; see: git-filter-repo or BFG"
HINTS[build_release]="fix compilation errors above; run: cargo build --workspace --release --locked"

launch() {
    local name="$1"; shift
    ORDER+=("$name")
    PENDING+=("$name")
    local log="${WORKDIR}/${name}.log"
    local sf="${WORKDIR}/${name}.status"
    (
        "$@" > "$log" 2>&1
        echo $? > "$sf"
    ) &
    PIDS["$name"]=$!
    printf "${DIM}  %-14s started${RESET}\n" "$name"
}

mark_skip() {
    local name="$1" reason="$2"
    ORDER+=("$name")
    STATUS["$name"]=skip
    echo -e "\n${YELLOW}${BOLD}┌─ $name${RESET}"
    echo -e "${YELLOW}│${RESET}  skipped — $reason"
    echo -e "${YELLOW}${BOLD}└─ SKIP${RESET}"
}

print_section() {
    local name="$1"
    local code="${STATUS[$name]}"
    local log="${WORKDIR}/${name}.log"
    local color label
    if [[ "$code" -eq 0 ]]; then color="$GREEN"; label="PASS"
    else                          color="$RED";   label="FAIL"
    fi

    echo -e "\n${color}${BOLD}┌─ $name${RESET}"
    if [[ -f "$log" && -s "$log" ]]; then
        while IFS= read -r line; do
            printf "${color}│${RESET}  %s\n" "$line"
        done < "$log"
    fi
    echo -e "${color}${BOLD}└─ $label${RESET}"
    if [[ "$code" -ne 0 && -n "${HINTS[$name]:-}" ]]; then
        echo -e "   ${DIM}hint: ${HINTS[$name]}${RESET}"
    fi
}

flush_completed() {
    local still_pending=()
    for name in "${PENDING[@]+"${PENDING[@]}"}"; do
        local sf="${WORKDIR}/${name}.status"
        if [[ -f "$sf" ]]; then
            wait "${PIDS[$name]}" 2>/dev/null || true
            STATUS["$name"]=$(cat "$sf")
            print_section "$name"
        else
            still_pending+=("$name")
        fi
    done
    PENDING=("${still_pending[@]+"${still_pending[@]}"}")
}

wait_for() {
    while true; do
        flush_completed
        local all_done=true
        for name in "$@"; do
            [[ "${STATUS[$name]+_}" ]] || { all_done=false; break; }
        done
        $all_done && break
        sleep 0.1
    done
}

gate_ok() {
    for name in "$@"; do
        local s="${STATUS[$name]:-1}"
        [[ "$s" =~ ^[0-9]+$ && "$s" -eq 0 ]] || return 1
    done
}

banner "Wave 1 — parallel: rustfmt | clippy | test | selectable_text | gitleaks"

launch rustfmt bash -c '
    cargo fmt --all
    if ! git diff --quiet --exit-code -- . ":(exclude)Cargo.lock"; then
        echo "Formatting updated tracked files."
    else
        echo "all files already properly formatted"
    fi
'

launch clippy cargo clippy --workspace --all-targets --locked -- -D warnings
launch test cargo test --workspace --locked
launch selectable_text bash scripts/check-selectable-text.sh

if has_tool gitleaks; then
    launch gitleaks gitleaks detect --source . -v
else
    mark_skip gitleaks "not installed — https://github.com/gitleaks/gitleaks#installing"
fi

banner "Gate — rustfmt | clippy | test | selectable_text"
wait_for rustfmt clippy test selectable_text

if gate_ok rustfmt clippy test selectable_text; then
    banner "Wave 2 — cargo_audit"
    if cargo audit --version >/dev/null 2>&1; then
        launch cargo_audit cargo audit
    else
        echo "  Installing cargo-audit..."
        cargo install cargo-audit --locked >/dev/null
        launch cargo_audit cargo audit
    fi

    banner "Gate — cargo_audit"
    wait_for cargo_audit

    if gate_ok cargo_audit; then
        banner "Wave 3 — build_release"
        launch build_release cargo build --workspace --release --locked
        wait_for build_release
    else
        mark_skip build_release "cargo_audit failed"
    fi
else
    mark_skip cargo_audit "lint/test gate failed"
    mark_skip build_release "lint/test gate failed"
fi

if [[ ${#PENDING[@]} -gt 0 ]]; then
    banner "Waiting for remaining checks…"
    while [[ ${#PENDING[@]} -gt 0 ]]; do
        flush_completed
        [[ ${#PENDING[@]} -gt 0 ]] && sleep 0.1
    done
fi

echo -e "\n${BOLD}══════════════════════════════════════════${RESET}"
echo -e "${BOLD} Pre-PR Summary${RESET}"
echo -e "${BOLD}══════════════════════════════════════════${RESET}\n"

any_failed=false
for name in "${ORDER[@]}"; do
    s="${STATUS[$name]:-?}"
    if   [[ "$s" == skip ]]; then
        printf "  ${YELLOW}SKIP${RESET}  %s\n" "$name"
    elif [[ "$s" =~ ^[0-9]+$ && "$s" -eq 0 ]]; then
        printf "  ${GREEN}PASS${RESET}  %s\n" "$name"
    else
        printf "  ${RED}FAIL${RESET}  %s\n" "$name"
        printf "        ${DIM}hint: %s${RESET}\n" "${HINTS[$name]:-see output above}"
        any_failed=true
    fi
done

echo ""
if $any_failed; then
    echo -e "${RED}${BOLD}Fix the issues above before opening a PR.${RESET}"
    exit 1
else
    echo -e "${GREEN}${BOLD}All actions completed. Safe to open a PR.${RESET}"
    exit 0
fi
