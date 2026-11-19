# shellcheck shell=bash
# Shared by the legacy desktop QA runners.
prepare_fixture() {
  FIXTURE="${GUST_FIXTURE:-}"
  qa_run_dir=""
  if [[ -z "$FIXTURE" ]]; then
    qa_run_dir="$(mktemp -d /tmp/gust-fixture.XXXXXXXX)" || return 1
    # Full-suite sections also create sibling trees such as "${FIXTURE}-panes".
    FIXTURE="$qa_run_dir/home"
    echo "gust qa: using $FIXTURE"
    qa_run_dir_id="$(stat -c '%d:%i' -- "$qa_run_dir")" || return 1
  fi
  "$HERE/make-fixture.sh" "$FIXTURE" >/dev/null
}

# Both runners call this only after their final success check.
finish_fixture() {
  [[ -n "$qa_run_dir" ]] || return 0
  if [[ "${GUST_KEEP_FIXTURE:-0}" == 1 ]]; then
    echo "gust qa: kept $qa_run_dir"
    return 0
  fi
  # Freeze mount propagation across the check and removal. A same-device
  # bind mount would otherwise look local even to rm --one-file-system.
  if ! unshare --user --map-current-user --mount --propagation private \
      bash -s -- "$qa_run_dir" "$qa_run_dir_id" <<'CLEANUP'
set -eu
root="$1"
[[ "$(stat -c '%d:%i' -- "$root")" == "$2" ]] || exit 1
# The generated path is ASCII and needs no mountinfo unescaping.
awk -v root="$root" '$5 == root || index($5, root "/") == 1 { exit 1 }' /proc/self/mountinfo
rm -r --one-file-system -- "$root"
CLEANUP
  then
    echo "gust qa: could not safely clean $qa_run_dir; inspect the fixture" >&2
  fi
}
