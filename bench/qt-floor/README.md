# Disposable Qt Quick floor control

This experiment asks which GPU/driver costs appear in a minimal Qt Quick
window as well as Gust. It does not recommend a renderer or a UI migration.
Only this directory is source-controlled; builds, app homes, logs, raw `/proc`
captures and screenshots are placed in a fresh `/var/tmp` directory.

The measured control matches Qt, final window size, compositor, fixtures/count,
host and renderer selection. **It does not match Gust's font or row height.**
The control uses DejaVu Sans 14 px and 28 px rows; original Gust fallback uses
`monospace` 12 px and 26 px rows. On this host that resolves to JetBrainsMono
Nerd Font. The raw mappings show those respective font files. Gust also has
headers, icons and multiple font weights. Consequently text/atlas and delegate
costs cannot be isolated by subtracting these applications.

## Measured run, 2026-11-05

Raw evidence was retained privately; no logs, screenshots or machine paths are published.
`summary.md` contains the complete DRM counter matrix and per-path mapping
attribution. `summary.json` includes median/min/max values, `results.json`
contains every sample, and `manifest.json` binds binaries, sources and Qt
library hashes. `SHA256SUMS` seals the evidence files.

The 36-launch matrix completed: three launches for every application ×
empty/100k × OpenGL/Vulkan/software combination. Sampling starts at map+3 s.
Pairs alternate application order. All launches use new HOME/XDG state and
warm host caches. Gust is the parent's original release `c50e92a`, not the
candidate being rebuilt. Source revision is parent-supplied; binary hash was
independently read. No Cargo build ran.

Binary SHA-256:

```
gust:     3a302fd87b84676f9448024d9438587042bb9b80e44116cb7b76681de78e46b6
qt-floor: fef3316913962e8ff027e9e9793da21e231b51e8186b3f9560bce929dffcf047
```

Medians below are MiB. These are separate accounting views: do not add PSS to
DRM, sum resident/shared columns, or subtract shared counters to claim physical
ownership. Software had no application DRM client; dashes mean no exposed
application counter, not zero compositor GPU memory.

| Renderer | Case | App | mapping PSS | resident VRAM | shared VRAM | resident GTT | shared GTT |
|---|---|---|---:|---:|---:|---:|---:|
| OpenGL | empty | control | 81.34 | 21.40 | 5.02 | 8.02 | 0.00 |
| OpenGL | empty | Gust | 122.45 | 64.22 | 15.07 | 26.02 | 0.00 |
| OpenGL | 100k | control | 93.36 | 41.45 | 15.07 | 16.02 | 0.00 |
| OpenGL | 100k | Gust | 153.63 | 74.22 | 10.05 | 26.02 | 0.00 |
| Vulkan | empty | control | 38.26 | 90.82 | 20.09 | 34.62 | 0.00 |
| Vulkan | empty | Gust | 75.99 | 90.84 | 20.09 | 34.62 | 0.00 |
| Vulkan | 100k | control | 49.42 | 90.82 | 20.09 | 34.62 | 0.00 |
| Vulkan | 100k | Gust | 107.59 | 90.89 | 20.09 | 34.62 | 0.00 |
| software | empty | control | 24.97 | — | — | — | — |
| software | empty | Gust | 56.49 | — | — | — | — |
| software | 100k | control | 36.54 | — | — | — | — |
| software | 100k | Gust | 84.73 | — | — | — | — |

The high Vulkan DRM residency is already present in an **empty** minimal Qt
window: about 90.82 MiB resident VRAM, 20.09 MiB shared VRAM and 34.62 MiB
resident GTT. Gust exposes nearly the same counters despite higher CPU PSS.
That supports a substantial Qt/RADV window/driver floor on this setup; it
does not attribute allocations to individual Qt objects.

The OpenGL control already has significant DRM residency and a mapped Mesa
driver. Gust has higher counters, but this comparison cannot isolate their
causes. Both GL applications map `libgallium-26.2.6-arch1.1.so`: its 100k-case
mapping PSS medians are 10.27 MiB control and 11.20 MiB Gust. Vulkan maps
`libvulkan_radeon.so` at 9.86 and 10.48 MiB respectively. Software maps neither
of those libraries. `/dev/dri` mappings have zero reported PSS in these
samples, illustrating why CPU PSS does not describe all GPU residency.
Anonymous/heap PSS cannot be assigned to the driver versus Qt from smaps.

## Dimensions and matching evidence

- Headless labwc output was set to 1280×900 using `wlr-randr`; that command
  succeeded. All 12 screenshots have PNG dimensions 1280×900. Compositor
  maximized decorations are disabled, and inspected screenshots show app
  content filling the output.
- All 18 control logs independently report settled `size=1280x900` from the
  actual QQuickView. All six Gust Vulkan logs report the final recycled
  swapchain as 1280×900; the control creates its Vulkan swapchain at 1280×900.
  Gust initially creates at 1180×720 before maximization. This startup
  sequence differs and may affect retained allocation sizes.
- OpenGL Gust has no independent numeric swapchain-size message; full-output
  screenshots and the identical compositor rule corroborate its final size.
  A pixel buffer calculation is not a substitute for that evidence.
- A 5.02 MiB shared-region counter is not proof of a different pixel size.
  fdinfo reports client-region aggregates, not a buffer's width, stride,
  layout or metadata. Those BO details were not captured, so the allocation
  formula is unresolved; no exact swapchain/atlas byte attribution is claimed.

Qt is system **6.12.0**: qmake queries `/usr`, Gust build output links the
system modules, ldd resolves `/usr/lib`, and both applications' smaps show
`libQt6Quick.so.6.12.0`. Packages: qt6-base/declarative 6.12.0-3,
qt6-wayland 6.12.0-1, Mesa/RADV 26.2.6, libdrm 2.4.134, labwc 0.20.2.
GPU: amdgpu, PCI 0000:10:00.0 / 1002:164E, Ryzen 5 7600 integrated Radeon.
Kernel: 7.2.14-1-omarchy. Vulkan ICD is restricted per process to RADV;
OpenGL logs independently identify radeonsi on that same GPU. The private
compositor uses GLES2 on renderD129 for all arms, including software.

## Accounting and checks

Every live descendant is enumerated with psutil. Raw smaps, parsed individual
mappings and every DRM fdinfo are saved for each accessible process. The
matrix observed only the root processes, so descendant handling was not
exercised with an actual child in this run. Exited children are excluded.
All `/proc` reads succeeded. Mapping PSS is the sum of the individual rounded
smaps `Pss` fields, not smaps_rollup; small rounding differences from a rollup
are possible. Reads are sequential (20–32 ms per sample), not atomic.

DRM aliases are deduplicated by PCI device plus client ID across the entire
tree: 48 fd references became 24 client observations. First-observed counters
are used once; later variants are retained. All exposed memory, total,
resident, shared, active and purgeable regions remain separate, including
CPU/GTT/VRAM and zero-valued GDS/GWS/OA/doorbell/mmioremap. Counter aliases
are not combined. Shared buffers may also be reported by the compositor;
compositor ownership is neither inferred nor subtracted.

Observed checks: qmake/make and the C observer built successfully;
`analyze.py` passed all 36 samples (renderer logs, control dimensions/counts,
PNG dimensions, unique client keys, mapping PSS sums and zero `/proc` errors).
Representative GL, Vulkan and software screenshots were visually inspected.
The isolated compositor and apps exited, and the QA lock was released.
No native user-desktop input, paint-ready timing, scrolling, theming or
long-running residency behavior was tested. Native candidate verification is
the parent's separate work. No council delegates or scores were produced
here, following the explicit no-subagents constraint.

## Reproduce

Requires system Qt Quick development packages/qmake6, C/C++ compilers,
wayland-scanner/protocols, labwc, wlr-randr, grim, dbus-daemon and Python
psutil. Supply an existing binary and existing `empty/` and `100k/` fixture
directories; the control's model uses a lazy integer count, not a filesystem
listing or a materialized 100,000-string model.

```sh
# Set fresh caller-owned output and synthetic-fixture directories first.
python bench/qt-floor/run.py \
  --output "$RUN_DIR" \
  --gust /path/to/existing/gust \
  --fixtures "$FIXTURE_DIR" --rounds 3
python bench/qt-floor/analyze.py "$RUN_DIR"
```

The runner acquires `/tmp/gust-qa.lock` nonblocking after its qmake build and
exits if another QA run owns it. It creates a private runtime directory,
session bus, labwc configuration and per-app homes. It only terminates process
groups it launched. It never builds Cargo, changes renderer defaults, touches
user desktop/configuration, or kills existing processes. The measured font
is intentionally left unchanged in source to preserve evidence provenance;
a stricter future font-matched experiment must use its own fresh evidence.
