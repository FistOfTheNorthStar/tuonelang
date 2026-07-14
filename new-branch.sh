#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 ]]; then
  echo "Usage: ./new-branch.sh <your-branch-name>"
  echo "Example: ./new-branch.sh fix-navbar"
  exit 1
fi

DATE_PREFIX="$(date +%d-%m-%Y)"
SUFFIX="$1"

# Initialize max_id to 0.
# If no branches exist for today, the first one will be 0 + 1 = 1.
max_id=0

while IFS= read -r ref; do
  # Strip common remote prefix so `origin/xx` becomes `xx`
  b="${ref#origin/}"

  # Check if the branch starts with "DD-MM-YYYY-"
  if [[ "$b" == "$DATE_PREFIX-"* ]]; then

    # Remove the date prefix.
    # If b is "18-01-2026-5-fix-bug", rest becomes "5-fix-bug"
    rest="${b#"$DATE_PREFIX-"}"

    # Extract the number before the next hyphen.
    # If rest is "5-fix-bug", n becomes "5"
    n="${rest%%-*}"

    # Check if 'n' is a valid integer.
    if [[ "$n" =~ ^[0-9]+$ ]]; then
      # If this number is higher than what we've seen, update max_id
      if (( n > max_id )); then
        max_id="$n"
      fi
    fi
  fi

done < <(
  git for-each-ref --format='%(refname:short)' refs/heads refs/remotes \
    | grep -vE '^(HEAD|origin/HEAD)$' \
    | sort -u
)

# Always increment the found max_id by 1
NEXT_ID=$((max_id + 1))

# Construct new name: DD-MM-YYYY-N-suffix
BRANCH_NAME="${DATE_PREFIX}-${NEXT_ID}-${SUFFIX}"

# Safety check
if git show-ref --verify --quiet "refs/heads/$BRANCH_NAME"; then
  echo "❌ Local branch already exists: $BRANCH_NAME"
  exit 1
fi

git checkout -b "$BRANCH_NAME"
echo "✅ Created and switched to branch: $BRANCH_NAME"