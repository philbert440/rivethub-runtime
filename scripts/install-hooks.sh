#!/usr/bin/env bash
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
git config core.hooksPath scripts/git-hooks
echo "hooks installed: $(ls scripts/git-hooks | tr '\n' ' ')"
