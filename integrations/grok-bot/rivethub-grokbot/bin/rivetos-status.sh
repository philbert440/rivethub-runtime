#!/usr/bin/env bash
# Thin wrapper — implementation lives in integrations/shared/.
# Sourced (not exec'd) so `bash -x` on this path still traces status
# (onboard-status.test.sh xtrace leak checks).
_here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
_impl=""
for _c in \
  "$_here/../../../shared/rivetos-status.sh" \
  "${RIVETOS_ROOT:-/opt/rivetos}/integrations/shared/rivetos-status.sh"; do
  if [ -f "$_c" ]; then
    _impl="$_c"
    break
  fi
done
if [ -z "$_impl" ]; then
  echo "rivetos-status: shared helper not found" >&2
  exit 1
fi
# shellcheck source=../../../shared/rivetos-status.sh
. "$_impl"
