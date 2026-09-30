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

## Latest reproducible run (`just bench-field`, 2026-09-30, at 319fc4b)

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
| Low-end profile (2 vCPU / 2 GB / software) | **not run** — needs the VM re-provisioned; `PROFILE=lowend just bench-field` |
| First rows painted before Flea's first window | not measured |
| Zero blank frames on folder change | not measured |
| 100k scroll p95 ≤ 16.7 ms on software rendering | not measured |
| Context menu ≤ 16 ms, Quick Look JPEG faster than Flea | not measured |
| SMB 10k listing | not measured |

The media run uses Gust's default Rows view, which requests no thumbnails;
Grid thumbnail cost is not in these numbers. None of this proves GPU paths
(Flea's Vulkan-on-Haswell bug class): that is an owner hardware check.
