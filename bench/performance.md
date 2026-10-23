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
