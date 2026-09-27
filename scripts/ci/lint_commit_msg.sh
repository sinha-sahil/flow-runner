#!/usr/bin/env bash
# Lints one commit message against the rules in CONTRIBUTING.md#commits:
#
#   feat: add a thing, in lowercase and at most 72 characters
#
#   - area: what changed and why, at most 30 words a bullet
#   - area: at most 10 bullets; longer bullets continue on indented lines
#
# `!` after the type marks a breaking change. Comment lines are ignored.
set -uo pipefail

TYPES='feat|fix|perf|refactor|docs|test|build|ci|chore|style|revert'
SUBJECT_MAX=72
BULLET_MAX_WORDS=30
BULLETS_MAX=10

message="${1:?usage: lint_commit_msg.sh <message-file>}"

errors=()
bullet=""
bullet_line=0
bullets=0
line_no=0
lines_read=0

flush_bullet() {
  [[ -z "${bullet}" ]] && return
  local words
  words=$(printf '%s' "${bullet}" | wc -w | tr -d ' ')
  if ((words > BULLET_MAX_WORDS)); then
    errors+=("line ${bullet_line}: the bullet has ${words} words, more than ${BULLET_MAX_WORDS}: split or trim it")
  fi
  bullet=""
}

while IFS= read -r line || [[ -n "${line}" ]]; do
  line_no=$((line_no + 1))
  # `git commit --verbose` puts the diff below this line
  [[ "${line}" == "# ------------------------ >8 ------------------------" ]] && break
  [[ "${line}" == '#'* ]] && continue
  lines_read=$((lines_read + 1))

  if ((lines_read == 1)); then
    if [[ ! "${line}" =~ ^(${TYPES})!?:\ [a-z0-9] ]]; then
      errors+=("line ${line_no}: the subject must be 'type: summary', with type one of ${TYPES//|/, } and the summary in lowercase; got '${line}'")
    fi
    if ((${#line} > SUBJECT_MAX)); then
      errors+=("line ${line_no}: the subject has ${#line} characters, more than ${SUBJECT_MAX}")
    fi
  elif ((lines_read == 2)); then
    if [[ -n "${line}" ]]; then
      errors+=("line ${line_no}: leave a blank line between the subject and the body")
    fi
  elif [[ -z "${line}" ]]; then
    flush_bullet
  elif [[ "${line}" == '- '* ]]; then
    flush_bullet
    bullet="${line#- }"
    bullet_line=${line_no}
    bullets=$((bullets + 1))
  elif [[ "${line}" =~ ^[[:space:]] && -n "${bullet}" ]]; then
    bullet="${bullet} ${line}"
  else
    flush_bullet
    errors+=("line ${line_no}: the body is '- ' bullets only, with continuation lines indented; got '${line}'")
  fi
done <"${message}"
flush_bullet

if ((lines_read == 0)); then
  errors+=("the message is empty")
elif ((bullets == 0)); then
  errors+=("the body has no bullets: say what changed as '- area: ...' lines")
elif ((bullets > BULLETS_MAX)); then
  errors+=("the body has ${bullets} bullets, more than ${BULLETS_MAX}")
fi

# `"${errors[@]}"` of an empty array is an unbound-variable error in bash 3
if ((${#errors[@]} > 0)); then
  for error in "${errors[@]}"; do
    echo "FAIL ${error}"
  done
  exit 1
fi
