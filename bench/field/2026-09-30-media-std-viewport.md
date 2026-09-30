# Media fixture, standard profile — 2026-09-30

Fixture: `tools/flea-media-fixture` (2000 files) on btrfs, DROP_THUMBS=yes, 3 cold runs each, medians [min-max]. Gust at the P1 viewport build (a4541a1).

```
entrant     n        mapped ms         settled ms        PSS MiB  +helper  cpu_s cpu_tree
flea        3    364 [353-399]   1132 [1121-1229]  224 [224-229]      3.4   0.66     2.22
gust        3    289 [289-302]      445 [444-459]     90 [88-92]      0.0   0.27     0.29
nautilus    3    640 [600-643] 16639 [16481-18389]  313 [313-330]      0.0   8.54    67.57
thunar      3    319 [307-363]      634 [628-662]     41 [41-41]      0.0   0.45     0.51
dolphin     3    454 [428-455]  9609 [9294-11038]  108 [105-109]      0.0   3.13    38.44
strata      3    530 [486-656]   1818 [1620-1867]  192 [191-194]      0.0   0.80     3.13
flea-tui    3    215 [201-230]   1018 [1012-1435]        2 [2-2]      0.0   0.00     1.43
```

Gust leads every GUI entrant on first window, settled and CPU. **Thunar uses less memory (41 vs 90 MiB PSS)** — not beaten; recorded as an open item. flea-tui is a terminal client and is not a like-for-like window.

Caveat: Gust's default view is Rows, which asks for no thumbnails; Grid-view thumbnail cost is not in this number.
