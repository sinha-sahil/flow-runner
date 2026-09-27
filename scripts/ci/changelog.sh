#!/usr/bin/env bash
# Works on CHANGELOG.md's release sections:
#
#   changelog.sh stamp 1.3.0 2026-10-01   turn `## [Unreleased]` into the
#                                         section for 1.3.0, and open a new,
#                                         empty `## [Unreleased]` above it
#   changelog.sh notes 1.3.0              print the section for 1.3.0
set -euo pipefail

command="${1:?usage: changelog.sh stamp <version> <date> | notes <version>}"
version="${2:?usage: changelog.sh stamp <version> <date> | notes <version>}"
file="${CHANGELOG:-CHANGELOG.md}"

case "${command}" in
  stamp)
    date="${3:?usage: changelog.sh stamp <version> <date>}"
    if ! grep -q '^## \[Unreleased\]' "${file}"; then
      echo "changelog.sh: ${file} has no '## [Unreleased]' section" >&2
      exit 1
    fi
    awk -v heading="## [${version}] - ${date}" '
      !done && /^## \[Unreleased\]/ { print; print ""; print heading; done = 1; next }
      { print }
    ' "${file}" >"${file}.new"
    mv "${file}.new" "${file}"
    ;;
  notes)
    awk -v heading="## [${version}]" '
      index($0, heading) == 1 { inside = 1; next }
      inside && /^## \[/ { exit }
      inside && (started || NF) { started = 1; print }
    ' "${file}"
    ;;
  *)
    echo "changelog.sh: unknown command '${command}'" >&2
    exit 1
    ;;
esac
