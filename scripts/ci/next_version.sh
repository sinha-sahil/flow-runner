#!/usr/bin/env bash
# Prints the next version and what bumped it, from the current version and
# the subjects of the commits since the last release, read from stdin:
#
#   feat!: ...  a major release, 1.2.3 -> 2.0.0
#   feat: ...   a minor release, 1.2.3 -> 1.3.0
#   anything    a patch release, 1.2.3 -> 1.2.4
#
# The largest bump any commit asks for wins, so a trailing `docs:` commit
# can't turn a pending feature release into a patch.
set -euo pipefail

current="${1:?usage: next_version.sh <current-version> < commit-subjects}"
if [[ ! "${current}" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)$ ]]; then
  echo "next_version.sh: '${current}' isn't a version like 1.2.3" >&2
  exit 1
fi
major=${BASH_REMATCH[1]}
minor=${BASH_REMATCH[2]}
patch=${BASH_REMATCH[3]}
subjects=$(cat)

if grep -qE '^[a-z]+!:' <<<"${subjects}"; then
  echo "$((major + 1)).0.0 major"
elif grep -qE '^feat:' <<<"${subjects}"; then
  echo "${major}.$((minor + 1)).0 minor"
else
  echo "${major}.${minor}.$((patch + 1)) patch"
fi
