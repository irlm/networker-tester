#!/usr/bin/env bash
# effective-changed-files.sh BASE_REF — changed files vs BASE_REF, EXCLUDING
# files whose entire diff is the mechanical version bump.
#
# Why this exists: every PR must bump the version in five files (CI enforces
# it), so `git diff --name-only` always contains install.sh, install.ps1,
# Cargo.toml, Cargo.lock and Directory.Build.props — which made every
# path-based job filter useless. A frontend-only PR ran the full installer
# execution matrix (Linux stack round-trip, Windows installer execution, the
# bats and shellcheck suites) because one version-string line moved (PR #664:
# ~13 min of jobs for a one-line bump).
#
# A file is dropped ONLY when every +/- line of its diff matches the exact
# bump signature for that file. Anything else — including a Cargo.lock
# dependency bump, which always carries `checksum =` lines — keeps the file.
set -euo pipefail

BASE="${1:?usage: effective-changed-files.sh BASE_REF}"

# Per-file allowed diff-line patterns (ERE, matched against +/- lines).
allowed_pattern() {
    case "$1" in
        install.sh)            echo '^[-+]INSTALLER_VERSION="v[0-9.]+"' ;;
        install.ps1)           echo '^[-+]\$InstallerVersion = "v[0-9.]+"' ;;
        Cargo.toml)            echo '^[-+]version *= *"[0-9.]+"' ;;
        Cargo.lock)            echo '^[-+]version = "[0-9.]+"' ;;
        Directory.Build.props) echo '^[-+] *<Version>[0-9.]+</Version>' ;;
        *)                     echo '' ;;
    esac
}

git diff --name-only "$BASE"...HEAD | while IFS= read -r file; do
    pat="$(allowed_pattern "$file")"
    if [ -z "$pat" ]; then
        printf '%s\n' "$file"
        continue
    fi
    # +/- lines of this file's diff, minus the file headers (---/+++).
    other=$(git diff "$BASE"...HEAD -- "$file" \
        | grep -E '^[-+]' \
        | grep -vE '^(\+\+\+|---)' \
        | grep -cvE "$pat" || true)
    if [ "$other" != "0" ]; then
        printf '%s\n' "$file"
    fi
    # else: version-only — dropped
done
