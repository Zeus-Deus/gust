# shellcheck shell=bash
# Shared by the legacy desktop QA runners.
prepare_fixture() {
  qa_run_dir="$(mktemp -d /tmp/gust-fixture.XXXXXXXX)" || return 1
  qa_run_dir_id="$(stat -c '%d:%i' -- "$qa_run_dir")" || return 1
  FIXTURE="${GUST_FIXTURE:-$qa_run_dir/home}"
  echo "gust qa: using $FIXTURE (private workspace $qa_run_dir)"
  # Auxiliary cases write and remove whole trees, even with an explicit fixture home.
  PANEDIR="$qa_run_dir/panes"
  QLDIR="$qa_run_dir/peek"
  DELDIR="$qa_run_dir/perm"
  ARCDIR="$qa_run_dir/arch"
  BULKDIR="$qa_run_dir/bulk"
  OWDIR="$qa_run_dir/openwith"
  MODIR="$qa_run_dir/multiopen"
  CLIPDIR="$qa_run_dir/clip"
  "$HERE/make-fixture.sh" "$FIXTURE" >/dev/null || return $?
  # Admit the original spelling first. Match Gust's lexical path spelling,
  # keeping even a final newline in the admitted directory name.
  IFS= read -r -d '' FIXTURE < <(realpath -esz -- "$FIXTURE")
}

# Initial on-disk state for the full runner's two-pane transfer journey.
prepare_pane_fixture() {
  mkdir -p "$PANEDIR/paneL" "$PANEDIR/paneR" || return 1
  printf 'one\n' > "$PANEDIR/paneL/p1.txt" || return 1
  printf 'two\n' > "$PANEDIR/paneL/p2.txt"
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
