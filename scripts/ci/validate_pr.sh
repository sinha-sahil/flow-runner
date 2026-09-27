#!/usr/bin/env bash
# Checks a pull request before it merges: the branch is named
# `type/what-it-does`, and every commit on it passes lint_commit_msg.sh.
set -uo pipefail

TYPES='feat|fix|perf|refactor|docs|test|build|ci|chore|style|revert'

branch="${1:?usage: validate_pr.sh <branch> <base-branch>}"
base="${2:?usage: validate_pr.sh <branch> <base-branch>}"
lint="$(dirname "$0")/lint_commit_msg.sh"
failed=0

echo "==> Branch ${branch}"
if [[ "${branch}" =~ ^(${TYPES})/[a-z0-9][a-z0-9-]*$ ]]; then
  echo "    ok"
else
  echo "    FAIL: name the branch 'type/what-it-does', such as 'fix/token-checksum'"
  failed=1
fi

echo "==> Commits since origin/${base}"
git fetch --quiet --no-tags origin "${base}"
message=$(mktemp)
trap 'rm -f "${message}"' EXIT
for commit in $(git rev-list --no-merges --reverse "origin/${base}..HEAD"); do
  git log -1 --format=%B "${commit}" >"${message}"
  subject=$(head -n1 "${message}")
  if problems=$(bash "${lint}" "${message}"); then
    echo "    ok    ${commit:0:9} ${subject}"
  else
    echo "    FAIL  ${commit:0:9} ${subject}"
    while IFS= read -r problem; do
      echo "          ${problem}"
    done <<<"${problems}"
    failed=1
  fi
done

if ((failed)); then
  echo "==> Fix the problems above; CONTRIBUTING.md#commits has the rules."
  exit 1
fi
echo "==> The pull request is ready to merge."
