#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

MASTER_CRATE="${MASTER_CRATE:-aura}"
REMOTE="${REMOTE:-origin}"
CHANGED_BUMP_KIND="${CHANGED_BUMP_KIND:-patch}"
MASTER_BUMP_KIND="${MASTER_BUMP_KIND:-patch}"
SKIP_CRATES_RAW="${SKIP_CRATES:-}"
TAG_PATTERN='v[0-9]*.[0-9]*.[0-9]*'

# Directories under the workspace root that contain crate Cargo.toml files.
# Mirrors the [workspace.members] layout in the root Cargo.toml.
CRATE_DIRS=("crates" "plugins")

usage() {
    cat <<'EOF'
Usage: scripts/prepare-release.sh [options]

Fetches the latest remote release tag, bumps changed workspace crates, and
forces the master crate to the next release version.

Options:
  --master-crate <name>        Master crate to force-bump. Default: aura
  --changed-bump <kind>        patch|minor|major for changed crates. Default: patch
  --master-bump <kind>         patch|minor|major for the master crate. Default: patch
  --remote <name>              Git remote to fetch tags from. Default: origin
  --skip <name>                Crate to exclude from bumps even if changed.
                               May be repeated or comma-separated. Cannot
                               include the master crate.
  --no-fetch                   Skip 'git fetch --tags'
  --dry-run                    Show planned changes without editing files
  -h, --help                   Show this help

Environment overrides:
  MASTER_CRATE
  CHANGED_BUMP_KIND
  MASTER_BUMP_KIND
  REMOTE
  SKIP_CRATES                  Space- or comma-separated list of crates to skip

Notes:
  - Release tags are expected to look like vX.Y.Z.
  - Crate manifests are discovered from: crates/*/Cargo.toml and
    plugins/*/Cargo.toml (matches the workspace layout).
  - A crate counts as changed when files under its manifest directory differ
    from the latest release tag.
  - Commit creation, tagging, and tag push remain manual.
EOF
}

die() {
    echo "error: $*" >&2
    exit 1
}

require_tool() {
    command -v "$1" >/dev/null 2>&1 || die "required tool not found: $1"
}

is_valid_bump_kind() {
    case "$1" in
        patch|minor|major) return 0 ;;
        *) return 1 ;;
    esac
}

bump_version() {
    local version="$1"
    local kind="$2"
    local major minor patch

    IFS=. read -r major minor patch <<<"$version"
    [[ "$major" =~ ^[0-9]+$ && "$minor" =~ ^[0-9]+$ && "$patch" =~ ^[0-9]+$ ]] \
        || die "invalid semver version: $version"

    case "$kind" in
        patch) patch=$((patch + 1)) ;;
        minor)
            minor=$((minor + 1))
            patch=0
            ;;
        major)
            major=$((major + 1))
            minor=0
            patch=0
            ;;
        *) die "unsupported bump kind: $kind" ;;
    esac

    printf '%s.%s.%s\n' "$major" "$minor" "$patch"
}

compare_versions() {
    local left="$1"
    local right="$2"
    local l_major l_minor l_patch r_major r_minor r_patch

    IFS=. read -r l_major l_minor l_patch <<<"$left"
    IFS=. read -r r_major r_minor r_patch <<<"$right"

    for pair in \
        "$l_major $r_major" \
        "$l_minor $r_minor" \
        "$l_patch $r_patch"; do
        set -- $pair
        if (( $1 < $2 )); then
            printf '%s\n' -1
            return 0
        fi
        if (( $1 > $2 )); then
            printf '%s\n' 1
            return 0
        fi
    done

    printf '%s\n' 0
}

latest_release_tag() {
    git tag --list "$TAG_PATTERN" --sort=-version:refname | head -n1
}

crate_manifest_for_name() {
    local crate_name="$1"
    local manifest
    while IFS= read -r manifest; do
        [[ -n "$manifest" ]] || continue
        if [[ "$(crate_name_from_manifest "$manifest")" == "$crate_name" ]]; then
            printf '%s\n' "$manifest"
            return 0
        fi
    done < <(list_workspace_crate_manifests)
    die "could not find Cargo.toml for crate: $crate_name"
}

crate_name_from_manifest() {
    sed -nE 's/^name[[:space:]]*=[[:space:]]*"(.*)"[[:space:]]*$/\1/p' "$1" | head -n1
}

crate_version_from_manifest() {
    sed -nE 's/^version[[:space:]]*=[[:space:]]*"(.*)"[[:space:]]*$/\1/p' "$1" | head -n1
}

set_manifest_version() {
    local manifest="$1"
    local new_version="$2"

    perl -0pi -e 's/^(version\s*=\s*")[^"]*(")/$1'"$new_version"'$2/m' "$manifest"
}

# Syncs the version inside the [workspace.dependencies] inline-table for a
# given crate, e.g.:  aura-core = { path = "...", version = "OLD" }  ->  "NEW"
# No-ops silently for crates without an entry there.
#
# Uses the /e modifier so the replacement is evaluated as Perl code with $1/$2
# variables — avoids the \10 octal-escape bug that occurs when \1 is followed
# by a digit (all our versions start with 0).
set_workspace_dep_version() {
    local crate_name="$1"
    local new_version="$2"

    perl -0pi -e 'my $v="'"$new_version"'"; s/^('"$crate_name"'\s*=\s*\{[^}]*version\s*=\s*")[^"]*(")/my($p,$s)=($1,$2);"$p$v$s"/gme' Cargo.toml
}

crate_changed_since_tag() {
    local tag="$1"
    local manifest="$2"
    local crate_dir

    crate_dir="$(dirname "$manifest")"
    if git diff --quiet "$tag"..HEAD -- "$crate_dir"; then
        return 1
    fi
    return 0
}

list_workspace_crate_manifests() {
    local d
    for d in "${CRATE_DIRS[@]}"; do
        [[ -d "$d" ]] || continue
        rg --files "$d" 2>/dev/null | rg '/Cargo\.toml$'
    done | sort -u
}

FETCH_TAGS=true
DRY_RUN=false

while [[ $# -gt 0 ]]; do
    case "$1" in
        --master-crate)
            [[ $# -ge 2 ]] || die "--master-crate requires a value"
            MASTER_CRATE="$2"
            shift 2
            ;;
        --changed-bump)
            [[ $# -ge 2 ]] || die "--changed-bump requires a value"
            CHANGED_BUMP_KIND="$2"
            shift 2
            ;;
        --master-bump)
            [[ $# -ge 2 ]] || die "--master-bump requires a value"
            MASTER_BUMP_KIND="$2"
            shift 2
            ;;
        --remote)
            [[ $# -ge 2 ]] || die "--remote requires a value"
            REMOTE="$2"
            shift 2
            ;;
        --skip)
            [[ $# -ge 2 ]] || die "--skip requires a value"
            SKIP_CRATES_RAW+=" $2"
            shift 2
            ;;
        --no-fetch)
            FETCH_TAGS=false
            shift
            ;;
        --dry-run)
            DRY_RUN=true
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            die "unknown option: $1"
            ;;
    esac
done

require_tool git
require_tool rg
require_tool perl
require_tool cargo

is_valid_bump_kind "$CHANGED_BUMP_KIND" || die "invalid changed bump kind: $CHANGED_BUMP_KIND"
is_valid_bump_kind "$MASTER_BUMP_KIND" || die "invalid master bump kind: $MASTER_BUMP_KIND"

declare -A SKIP_CRATES_SET=()
if [[ -n "$SKIP_CRATES_RAW" ]]; then
    skip_normalized="${SKIP_CRATES_RAW//,/ }"
    for name in $skip_normalized; do
        [[ -n "$name" ]] || continue
        if [[ "$name" == "$MASTER_CRATE" ]]; then
            die "cannot skip the master crate: $MASTER_CRATE"
        fi
        crate_manifest_for_name "$name" >/dev/null
        SKIP_CRATES_SET[$name]=1
    done
fi

if $FETCH_TAGS; then
    git fetch --tags "$REMOTE"
fi

LATEST_TAG="$(latest_release_tag)"
[[ -n "$LATEST_TAG" ]] || die "no release tags matching $TAG_PATTERN were found"

LATEST_VERSION="${LATEST_TAG#v}"
MASTER_TARGET_VERSION="$(bump_version "$LATEST_VERSION" "$MASTER_BUMP_KIND")"
MASTER_MANIFEST="$(crate_manifest_for_name "$MASTER_CRATE")"
MASTER_CURRENT_VERSION="$(crate_version_from_manifest "$MASTER_MANIFEST")"

if [[ "$(compare_versions "$MASTER_CURRENT_VERSION" "$MASTER_TARGET_VERSION")" == "1" ]]; then
    die "master crate $MASTER_CRATE already has newer version $MASTER_CURRENT_VERSION than target $MASTER_TARGET_VERSION"
fi

echo "Latest release tag: $LATEST_TAG"
echo "Latest release version: $LATEST_VERSION"
echo "Master crate: $MASTER_CRATE -> $MASTER_TARGET_VERSION"
if [[ ${#SKIP_CRATES_SET[@]} -gt 0 ]]; then
    printf 'Skip list: %s\n' "${!SKIP_CRATES_SET[*]}"
fi
echo ""

declare -a changed_manifests=()
declare -a skipped_changed=()

while IFS= read -r manifest; do
    [[ -n "$manifest" ]] || continue
    if [[ "$manifest" == "$MASTER_MANIFEST" ]]; then
        continue
    fi
    crate_name="$(crate_name_from_manifest "$manifest")"
    if [[ -n "${SKIP_CRATES_SET[$crate_name]:-}" ]]; then
        if crate_changed_since_tag "$LATEST_TAG" "$manifest"; then
            skipped_changed+=("$crate_name")
        fi
        continue
    fi
    if crate_changed_since_tag "$LATEST_TAG" "$manifest"; then
        changed_manifests+=("$manifest")
    fi
done < <(list_workspace_crate_manifests)

if [[ ${#changed_manifests[@]} -eq 0 ]]; then
    echo "No crate path changes detected since $LATEST_TAG."
else
    echo "Changed crates since $LATEST_TAG:"
    for manifest in "${changed_manifests[@]}"; do
        crate_name="$(crate_name_from_manifest "$manifest")"
        current_version="$(crate_version_from_manifest "$manifest")"
        next_version="$(bump_version "$current_version" "$CHANGED_BUMP_KIND")"
        printf '  %s: %s -> %s\n' "$crate_name" "$current_version" "$next_version"
    done
    echo ""
fi

if [[ ${#skipped_changed[@]} -gt 0 ]]; then
    echo "Skipped (changed but excluded by --skip):"
    for crate_name in "${skipped_changed[@]}"; do
        printf '  %s\n' "$crate_name"
    done
    echo ""
fi

if $DRY_RUN; then
    echo "Dry run only. No files were modified."
    exit 0
fi

for manifest in "${changed_manifests[@]}"; do
    current_version="$(crate_version_from_manifest "$manifest")"
    next_version="$(bump_version "$current_version" "$CHANGED_BUMP_KIND")"
    if [[ "$current_version" != "$next_version" ]]; then
        crate_name="$(crate_name_from_manifest "$manifest")"
        set_manifest_version "$manifest" "$next_version"
        set_workspace_dep_version "$crate_name" "$next_version"
    fi
done

if [[ "$MASTER_CURRENT_VERSION" != "$MASTER_TARGET_VERSION" ]]; then
    set_manifest_version "$MASTER_MANIFEST" "$MASTER_TARGET_VERSION"
    set_workspace_dep_version "$MASTER_CRATE" "$MASTER_TARGET_VERSION"
fi

cargo update --workspace

echo "Updated manifests:"
if [[ ${#changed_manifests[@]} -eq 0 ]]; then
    echo "  no changed crates required a bump"
else
    for manifest in "${changed_manifests[@]}"; do
        crate_name="$(crate_name_from_manifest "$manifest")"
        crate_version="$(crate_version_from_manifest "$manifest")"
        printf '  %s: %s\n' "$crate_name" "$crate_version"
    done
fi
echo "  $MASTER_CRATE: $(crate_version_from_manifest "$MASTER_MANIFEST")"
echo "  Cargo.toml [workspace.dependencies] version entries synced."
echo ""
echo "Next manual steps:"
echo "  1. Review manifest and Cargo.lock changes."
echo "  2. Commit the version bumps."
echo "  3. Create and push the release tag manually."
