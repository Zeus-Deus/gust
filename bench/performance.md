# Directory and thumbnail performance

Large directories avoid repeated numeric parsing during natural sorting.
Prepared keys have a 16 MiB scratch budget; larger or digit-heavy inputs keep
the stable comparator. Already ordered and strictly reversed runs avoid keys.
Printable ASCII display names avoid per-character conversion.

Thumbnail workers prioritize the visible window, admit one helper per model,
and retain only the latest pending viewport. Hidden views release their view
models and stop reporting viewport work. Live changes invalidate published
paths before refreshing pixels.

Cache hits read at most 512 bytes of PNG metadata. JPEG helpers decode near the
requested size. Source version checks include device, inode, length, and
nanosecond modification/change times; old cache entries regenerate once.
Decoding remains inside the existing sandbox.

## Grid PNG display memory

Grid decodes are bounded to the normal cache's 128-pixel box. A separate image
item fits the displayed rectangle without enlarging the decoded PNG. On Qt
6.12, the previous `PreserveAspectFit` decode policy loaded a 128×96 PNG as
341×256 for a 256-pixel requested box. The new item retains 128×96 pixels and
fits its displayed geometry, including zoom and non-square previews.

Five alternating release pairs on the same 2,000-JPEG fixture and real
2536×1390 desktop viewport measured median total process PSS **384.6 →
271.2 MiB (29.5% lower)**. Every run generated the same 363 cache files.
These observations use fresh application/thumbnail caches and warm OS caches;
PSS was sampled ten seconds after compositor mapping and excludes GPU memory.
This longer window lets both variants finish their original prefetch work.

A separate diagnostic pair observed 310 loaded image items: **27,061,760 →
3,809,280 decoded pixels**, with all 220 visible thumbnails ready in both
variants. First full-grid frames were sampled at approximately 3.6 seconds
after QML load in each; this is not evidence of faster visible completion.
Ten-second image CPU totals were 9.02 → 8.74 seconds, with overlapping ranges;
no general CPU-speed improvement is claimed. Small-folder mapping was
388 → 389 ms and PSS 175.4 → 175.8 MiB; its CPU median increased from
1.05 → 1.11 seconds with overlapping ranges. 100k mapping was 384 → 357 ms
and PSS 187.2 → 188.2 MiB. Shared-host timings describe this fixture/machine,
not a uniform whole-application gain.

The shipped image component is tested with landscape, portrait, panorama,
tall, oversized and recycled PNGs, resize/zoom without redecoding, and 1×/2×
display scales. Real-window checks cover full-grid rendering, zoom, reaching
photo 2000, live replacement pixels, and cancellation on leaving Grid or the
folder. Sandboxing, worker scheduling, freshness validation and the thumbnail
cache policy are unchanged.

```sh
python3 tests/grid-thumbnail.py
QT_SCALE_FACTOR=2 python3 tests/grid-thumbnail.py
```

## Folder and preview reuse

Local names can be reused from a process-local, 8 MiB / 16-location LRU after
worker-side directory identity and nanosecond timestamp checks. File metadata
is read again. Explicit Refresh bypasses the cache; remote/FUSE paths bypass
it entirely. There is no resident background process or persistent file index.

Grid previews use verified source-version URLs and Qt's decoded-image cache.
The URLs now address immutable version-specific copies, separate from the
replaceable freedesktop PNGs. Cache promotion checks the bytes through a held
descriptor; late concurrent publications cannot overwrite another version's
pixels. This uses additional disposable disk cache space.
Cache hits are attached before installing a listing when its Grid preference
is known, and published before any missing preview is rendered. Two render
workers share a process-wide two-helper limit. Cancellation retires the owned
helper group, and live refresh keeps unchanged URLs while clearing failed or
changed previews. The normal 128-pixel decode bound remains unchanged.

Three alternating release pairs on a generated 500-JPEG fixture (1600×1200
sources, 91 visible previews, isolated 1600×1000 software-rendered desktop)
observed median first-full-grid time **2.47 → 1.73 seconds** with empty thumbnail
caches. Warm new-process launches measured **438 → 368 ms**; Back navigation
measured **172 → 121 ms**. Every run displayed all 91 previews, generated the
same 156 cache files, and rewrote none on warm launches. Warm process PSS was
approximately 115 MiB in both variants, excluding GPU memory and external
services. The diagnostic sampled image readiness about every 100 ms; shared
host timing ranges overlap for warm launches and Back. These observations do
not establish a general startup-speed gain or promise instant uncached images.

A separate nine-round alternating worker measurement of 100,000 generated
names observed **32.9 → 1.8 ms** for enumeration/sorting versus validated name
reuse, with matching names, kinds and order. This excludes metadata, model
installation and painting; it is not a whole-directory-loading measurement.

Regression coverage includes LRU bounds, byte-safe ordering, membership changes,
explicit-refresh bypass, cache-first delivery, the two-worker/helper limits,
cancellation, versioned local image URLs and immediate decoded-pixel reuse.
Isolated desktop checks additionally verify same-second replacement pixels,
unchanged sibling cache files, failed-preview clearing, edits while away,
last-row previews and navigation cancellation. Captures and raw measurements
remain private and are not part of the source change.

Review regressions additionally cover coalesced file-to-directory replacements,
an old renderer publishing after a new version, and shared-cache replacement
while a promotion holds its descriptor. The timings above precede the immutable
publication follow-up; its correctness is checked separately.

## Prior release measurements

These observations come from paired synthetic-fixture comparisons of the
implementation before its port to current mainline. They are historical
evidence, rather than a new timing claim for every PR revision.

| Workload | Observed reduction in elapsed time |
| --- | ---: |
| Mixed 100,000-entry listing and first 256 metadata rows, 15 pairs | 26.8% |
| Final reverse-run variant on that mixed fixture, five samples per variant | 23.0% |
| Shuffled natural sorting, 15 pairs | 32.9% |
| Cold 4096×3072 JPEG grid thumbnail, two opposite-order comparisons | 74–81% |
| Grid thumbnail cache lookup | 21–25% |
| Quick Look thumbnail cache lookup | About 86% |

Page caches were warm; a cold thumbnail means its target cache file was removed
outside the timed interval. Ordering checksums matched. The ordinary populated
startup comparison with Automatic rendering improved 10.7%; these changes do
not establish a uniform 20% whole-application gain. Separate local Software
renderer tuning is outside this PR. Live connection latency was not measured.

Tradeoffs: the mixed listing benchmark used about 3 MiB more peak RSS; a
60-character-name comparator fallback case measured 5.5% slower in three
samples. Worst-case digit-heavy sort scratch matched the original comparator.
Cold PNG renders improved only 5–7%, and faster PNG compression increased one
measured large-preview cache file by about 21%. Full metadata loading was
essentially unchanged.

## Reproduction

Use generated files and a dedicated thumbnail cache directory. No personal
directory contents, captured desktop images, settings, or raw workstation
evidence are included here.

```sh
cargo run --release -p gust-core --example bench_listing -- GENERATED_DIRECTORY 15
cargo run --release -p gust-thumbs --example performance -- \
  PRIVATE_CACHE GENERATED_JPEG image/jpeg normal cold
```

Compare alternating release builds on the same fixture, preserve the raw output
locally, and verify ordering before interpreting the timing. The listing
example prints phase timings and an order checksum, without filenames.

## Content search in plain text

On a generated 32-file corpus, the median release time for the entire
`grep_with_report` walk fell from 129.7 ms to 65.3 ms. Nine alternating-order
pairs on the same warm tmpfs fixture all read 33,554,848 bytes and found all
32 files. The ranges were 125.1–151.4 ms and 62.0–87.6 ms. This measures the
Rust backend including file I/O, matching and snippet creation. The native
search journey separately checked activation, deferred rename and byte-exact
undo. The walk continues to report its existing partial-coverage status.

A second run compared the original corpus with an early hit at byte zero of
each file's single long ASCII line. Both corpora had the same file sizes and
query. The table shows whole-search medians and ranges in milliseconds for
nine alternating pairs per fixture on warm tmpfs.

| Fixture | Baseline (`f81702f`) | ASCII fast path (`1f178a0`) |
| --- | ---: | ---: |
| Short lines, match on the final line | 115.7 (111.8–127.4) | 59.4 (54.6–71.6) |
| Single long line, match at byte zero | 24.7 (23.8–26.6) | 25.0 (23.8–33.8) |

With the early hit, the candidate still scans the line to establish that it
is ASCII. Its median was 0.3 ms higher on this corpus, and the ranges overlap.

To repeat from the root of the candidate checkout, build the same example twice.
The baseline predates the example, so copy just its source into a detached
baseline worktree. Separate target directories keep both executables available.
The measurements used warm tmpfs. For the two corpora, choose a writable tmpfs
with at least 65 MiB free. The recipe defaults to `/dev/shm` and checks the
filesystem before building. The worktree and builds use the usual temporary
directory, which can be on disk.

```bash
set -eu
fixture_parent=${GUST_BENCH_TMPFS:-/dev/shm}
if [[ $(stat -f -c %T -- "$fixture_parent") != tmpfs ]]; then
    echo "Set GUST_BENCH_TMPFS to a writable tmpfs directory." >&2
    exit 1
fi
fixtures=$(mktemp -d -- "$fixture_parent/gust-content-search.XXXXXXXX")
run=$(mktemp -d)
git worktree add --detach "$run/baseline" f81702f11aad253e9dfdc541393de68cf225f897
cp crates/core/examples/bench_content_search.rs "$run/baseline/crates/core/examples/"
for build in baseline candidate; do
    source_dir=$PWD
    if [[ $build == baseline ]]; then source_dir="$run/baseline"; fi
    flock /tmp/gust-build.lock cargo build --locked --release \
        --manifest-path "$source_dir/Cargo.toml" --target-dir "$run/$build-target" \
        -p gust-core --example bench_content_search
done

python3 - "$fixtures" <<'PYTHON'
from pathlib import Path
import sys

root = Path(sys.argv[1])
line = b"Project notes about searching ordinary documentation and code.\n"
late = line * 16644 + b"Find NEEDLE here\n"
# One equally long file with the match at byte zero of a single ASCII line.
early = b"NEEDLE" + b"x" * (len(late) - 7) + b"\n"
for case, data in [("final-line", late), ("early-hit", early)]:
    folder = root / case
    folder.mkdir()
    for index in range(32):
        (folder / f"{index}.txt").write_bytes(data)
PYTHON

for fixture in final-line early-hit; do
    for round in {0..9}; do
        order=(baseline candidate)
        if (( round % 2 == 0 )); then order=(candidate baseline); fi
        for build in "${order[@]}"; do
            printf '%s %s %s ' "$fixture" "$round" "$build"
            "$run/$build-target/release/examples/bench_content_search" "$fixtures/$fixture" needle
        done
    done
done
```

Round zero warms both builds. Compare the median of the nine remaining runs
per build and fixture. Each run should report 32 hits and 33,554,848 bytes.
