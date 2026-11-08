#!/usr/bin/env bash
# Roll back only the failed GitHub Release and its tag.
# This personal repository deliberately does not create pull requests or mutate
# source branches as part of release cleanup.
# Expects env vars: TAG, REPO, GH_TOKEN
set -euo pipefail

TAG="${TAG:?TAG is required}"
REPO="${REPO:?REPO is required}"

echo "=== ROLLBACK: Failed release ==="
echo "Repository: $REPO"
echo "Tag: $TAG"

echo "Deleting GitHub Release (if one was created)..."
gh release delete "$TAG" --yes --repo "$REPO" 2>/dev/null || true

echo "Deleting remote tag (if one was pushed)..."
git push origin --delete "$TAG" 2>/dev/null || true

echo "Rollback complete. No source branch was changed and no pull request was created."
