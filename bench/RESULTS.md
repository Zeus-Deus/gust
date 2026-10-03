# Bench results

Method: Flea's own field bench (`tools/flea-field-bench`, thisisgm/flea
`docs/benchmarks.md`), run unmodified except for a 13-line diff that adds Gust
as an entrant (`bench/harness/gust-entrant.diff`). Same VM, same fixtures,
three cold runs per entrant, medians with [min–max].

- **First window** = compositor-mapped. **Settled** = 500 ms with no CPU
  activity in the entrant's process tree. **PSS** = window process;
  **+helper** = Flea's backend. **CPU** = whole tree.
- Fixtures live outside `$HOME` on btrfs: 100 000 empty files
  (`tools/flea-bench`) and 2 000 mixed media files (`tools/flea-media-fixture`).
- VM: KVM guest, 8 vCPU, 3 GiB, Omarchy on Hyprland, GPU-less (llvmpipe).
- Versions: Flea 0.3.6 (source, 98404bc), Strata 0.20.1, Nautilus, Dolphin and
  Thunar from the Arch repos at run time. Full manifests sit next to each CSV.
- Reproduce: `FLEA_REPO=~/src/flea just bench-field` (see `bench/field-bench.sh`).

## v1 re-bench against Flea 0.3.7 (`just bench-field`, 2026-10-03, at ac7b8c4)

Flea 0.3.7 from source (ae9419c, release build), Strata 0.20.1, Nautilus
50.3.1, Dolphin 26.08.0, Thunar 4.20.9. Same VM as above (KVM, 8 vCPU,
3 GiB, llvmpipe, no GPU). CSVs and manifests: `bench/field/2026-10-03-*`.

**Low-end profile** (`PROFILE=lowend just bench-field`): the whole run is
pinned to 2 CPUs (`taskset -c 0,1`, inherited by every entrant), placed in a
user scope with `MemoryMax=2G` and swap off, and every Qt entrant gets
`QT_QUICK_BACKEND=software`. Caches are dropped before every run on both
profiles (cold cache). The machine itself was not resized: a Realm guest
cannot change its own vCPU count, so the limits are applied to the run.
Checked on the live harness process: affinity `0,1`, `memory.max`
2147483648, `QT_QUICK_BACKEND=software` in its environment.

### Standard profile (medians of 3 cold runs, [min–max])

| Fixture | Entrant | First window ms | Settled ms | PSS MiB | CPU s |
|---|---|---|---|---|---|
| 100k | **Gust** | **240** [234–258] | **410** [340–411] | **93** | **0.24** |
| 100k | Flea 0.3.7 | 297 [295–313] | 756 [748–769] | 177 (+6.9) | 0.36 |
| 100k | Strata | 363 | 762 | 198 | 0.67 |
| 100k | Dolphin | 350 | 2876 | 208 | 3.97 |
| 100k | Nautilus | 475 | 4051 | 347 | 4.01 |
| 100k | Thunar | 275 | 11726 | 363 | 11.67 |
| media | **Gust** | **228** [227–266] | **277** [276–291] | 80 | **0.19** |
| media | Flea 0.3.7 | 310 [295–315] | 848 [812–1120] | 186 (+3.4) | 0.47 |
| media | Thunar | 267 | 567 | **39** | 0.38 |
| media | Strata | 361 | 1282 | 160 | 0.67 |
| media | Dolphin | 367 | 7788 | 97 | 2.71 |
| media | Nautilus | 493 | 14205 | 279 | 7.79 |

Gust leads every column of Flea's table on both fixtures.

### Low-end profile (2 CPUs, 2 GiB, software rendering, cold cache)

| Fixture | Entrant | First window ms | Settled ms | PSS MiB | CPU s |
|---|---|---|---|---|---|
| 100k | Gust | 217 [205–232] | **352** [348–365] | 93 | 0.22 |
| 100k | Flea 0.3.7 | **211** [205–219] | 434 [354–741] | **75** (+6.9) | **0.18** |
| 100k | Strata | 362 | 785 | 195 | 0.62 |
| 100k | Dolphin | 357 | 2748 | 209 | 2.87 |
| 100k | Nautilus | 490 | 3976 | 348 | 3.74 |
| 100k | Thunar | 251 | 11309 | 363 | 11.26 |
| media | Gust | 242 [228–273] | **321** [319–425] | 76 | **0.19** |
| media | Flea 0.3.7 | **204** [201–217] | 983 [975–988] | **74** (+3.4) | 0.20 |
| media | Thunar | 251 | 551 | **39** | 0.35 |
| media | Strata | 362 | 2045 | 162 | 0.56 |
| media | Dolphin | 359 | 15788 | 94 | 2.23 |
| media | Nautilus | 464 | 28171 | 280 | 5.46 |

**Where Gust does not lead on the low-end profile, and why (data, not a fix):**

- *Why Flea moved and Gust did not.* Software rendering is what changed
  Flea's numbers: first window 297 → 211 ms and PSS 177 → 75 MiB on 100k
  (media 310 → 204 ms, 186 → 74 MiB). That is consistent with Flea's
  Quickshell window otherwise rendering through llvmpipe GL on this
  GPU-less machine (inferred from the numbers, not traced). Gust already
  picks Qt's software renderer when it finds no GPU (renderer
  auto-selection, STATUS), so the profile takes nothing away from it:
  240 → 217 ms and 93 → 93 MiB. The standard-profile gap was Flea's GL
  context on llvmpipe, not Gust's work.
- *First window, 6 ms (100k) and 38 ms (media).* The 100k ranges overlap
  (Gust 205–232, Flea 205–219). The media gap is real. Gust still settles
  first by 82 ms (100k) and 662 ms (media): the window is usable sooner.
- *PSS on 100k, 93 vs 75 (+6.9 helper) MiB.* Gust holds the listing in
  the window process; Flea keeps it in a separate backend and the window
  gets only the visible rows. Measured with the release binary, software
  renderer, pinned to 2 CPUs (window process, `smaps_rollup`, 2 runs
  each): empty folder 53–57 MiB (16 MiB anon), 100k folder 77–80 MiB
  (39–40 MiB anon), media folder 62–66 MiB (25 MiB anon). About 23 MiB of
  the 100k figure is the listing itself. Window + helper: Flea 82 MiB,
  Gust 93 MiB on 100k; on media Gust is lower (76 vs 77).
- *CPU on 100k, 0.22 vs 0.18 s.* Gust lists and sorts in the window
  process; Flea's backend does that work outside it, so its window-only
  CPU is lower while its whole-tree CPU is higher (0.30 vs 0.23 s).

Closing these would mean moving the listing out of the window process.
That is an architecture change, not a v1 item; it is recorded in
`docs/HANDOFF.md` under "After v1".

## Earlier run (`just bench-field`, 2026-09-30, at 319fc4b, Flea 0.3.6)

CSVs: `bench/field/2026-09-30-scale-std.csv`, `bench/field/2026-09-30-media-std.csv`
(medians over three cold runs; the table below this one is the first run at a4541a1).

| Fixture | Entrant | First window | Settled | PSS MiB | CPU s |
|---|---|---|---|---|---|
| 100k | **Gust** | **212** [209–227] | **347** [333–381] | **109** | **0.28** |
| 100k | Flea 0.3.6 | 295 [291–299] | 757 [751–769] | 220 (+6.8) | 0.53 |
| 100k | Thunar | 251 | 11320 | 365 | 11.37 |
| 100k | Dolphin | 349 | 2857 | 218 | 3.81 |
| 100k | Strata | 392 | 844 | 234 | 0.78 |
| 100k | Nautilus | 506 | 3904 | 370 | 3.92 |
| media | **Gust** | **240** [240–247] | **359** [333–449] | 95 | **0.25** |
| media | Thunar | 243 [235–253] | 540 | **42** | 0.42 |
| media | Flea 0.3.6 | 298 [285–312] | 941 [899–950] | 227 (+3.3) | 1.88 |
| media | Strata | 386 | 1277 | 191 | 2.51 |
| media | Dolphin | 346 | 6823 | 109 | 30.20 |
| media | Nautilus | 501 | 12843 | 315 | 54.35 |

Gust leads every column of Flea's table on both fixtures in both runs. The
only column Gust does not win overall is media-fixture PSS, where Thunar
(GTK3, no Qt Quick scene graph) uses 42 MiB to Gust's 95.

## First run, 100 000 files, standard profile

| Entrant | First window | Settled | PSS MiB | CPU s |
|---|---|---|---|---|
| **Gust** | **269** [265–295] | **476** [429–580] | **107** | **0.34** |
| Flea 0.3.6 | 360 [352–418] | 816 [812–870] | 216 (+6.8) | 0.61 |
| Strata | 572 [453–613] | 1088 [937–1130] | 236 | 0.88 |
| Dolphin | 421 [397–606] | 3195 [3189–3485] | 221 | 4.44 |
| Nautilus | 629 [618–761] | 4688 [4667–5893] | 369 | 4.49 |
| Thunar | 317 [296–359] | 12842 [12742–14292] | 365 | 12.85 |
| flea-tui (terminal, not like-for-like) | 174 | 504 | 2 | 0.51 |

Gust before the fix (`4c2ac56`): 393 ms / 2098 ms / 115 MiB / 1.29 s — it
stat'ed and sorted all 100k entries before settling. Now it reads details for
the rows on screen (DECISIONS D6).

## 2 000 media files, standard profile

| Entrant | First window | Settled | PSS MiB | CPU s |
|---|---|---|---|---|
| **Gust** | **289** [289–302] | **445** [444–459] | 90 | **0.29** |
| Thunar | 319 [307–363] | 634 [628–662] | **41** | 0.51 |
| Flea 0.3.6 | 364 [353–399] | 1132 [1121–1229] | 224 (+3.4) | 2.22 |
| Dolphin | 454 [428–455] | 9609 [9294–11038] | 108 | 38.44 |
| Strata | 530 [486–656] | 1818 [1620–1867] | 192 | 3.13 |
| Nautilus | 640 [600–643] | 16639 [16481–18389] | 313 | 67.57 |

## Against the P1 targets

| Target | State |
|---|---|
| Lead every column of Flea's table, 100k | **met** |
| Lead every column of Flea's table, media | **met** vs Flea; Thunar has lower PSS (41 vs 90 MiB) |
| Low-end profile (2 vCPU / 2 GB / software) | **run 2026-10-03** vs Flea 0.3.7: Gust leads settled on both fixtures; Flea leads first window (6 / 38 ms) and window PSS on 100k — explained above |
| First rows painted before Flea's first window | not measured |
| Zero blank frames on folder change | not measured |
| 100k scroll p95 ≤ 16.7 ms on software rendering | not measured |
| Context menu ≤ 16 ms, Quick Look JPEG faster than Flea | not measured |
| SMB 10k listing | not measured |

The media run uses Gust's default Rows view, which requests no thumbnails;
Grid thumbnail cost is not in these numbers. None of this proves GPU paths
(Flea's Vulkan-on-Haswell bug class): that is an owner hardware check.
