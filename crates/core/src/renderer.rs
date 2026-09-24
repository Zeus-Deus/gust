//! Which Qt Quick renderer to use.
//!
//! On a machine with no usable GPU, Mesa falls back to llvmpipe: OpenGL
//! emulated on the CPU through LLVM. Measured in the no-GPU VM (release
//! build, 100k-entry folder): llvmpipe costs ~210 MiB PSS and a 155 ms first
//! window; Qt's own raster renderer (`software`) costs ~86 MiB and 104 ms.
//! So where there is no GPU, Gust asks Qt for the raster renderer.
//!
//! The decision reads sysfs only — no GL context is created to find out,
//! because creating one is exactly what loads llvmpipe.
//!
//! Rules, in order:
//! 1. `GUST_RENDERER=software|gpu` decides outright.
//! 2. If the user set `QT_QUICK_BACKEND` or `QSG_RHI_BACKEND`, respect it.
//! 3. A render node with a known hardware driver → GPU.
//! 4. A virtio GPU whose virgl (3D) feature bit is set → GPU.
//! 5. A virtio GPU without virgl, or no render node at all → software.
//! 6. Anything unrecognised → leave Qt's default. Never guess.

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Renderer {
    /// Qt's default (hardware RHI where available).
    Default,
    /// Qt Quick's raster renderer.
    Software,
}

/// DRM drivers that mean a real GPU with 3D acceleration.
const HARDWARE_DRIVERS: &[&str] = &[
    "i915", "xe", "amdgpu", "radeon", "nouveau", "nvidia", "nvidia-drm", "msm", "panfrost",
    "panthor", "lima", "v3d", "vc4", "etnaviv", "asahi", "powervr", "imagination", "mali",
    "tegra", "vmwgfx",
];

/// Decide from an environment lookup and a sysfs root (normally `/sys`).
pub fn choose(env: impl Fn(&str) -> Option<String>, sys: &Path) -> Renderer {
    match env("GUST_RENDERER").as_deref().map(str::trim) {
        Some("software") => return Renderer::Software,
        Some("gpu") | Some("hardware") => return Renderer::Default,
        _ => {}
    }
    for var in ["QT_QUICK_BACKEND", "QSG_RHI_BACKEND"] {
        if env(var).is_some_and(|v| !v.trim().is_empty()) {
            return Renderer::Default;
        }
    }

    let Ok(rd) = std::fs::read_dir(sys.join("class/drm")) else {
        // No DRM class at all: cannot tell (containers, odd kernels).
        return Renderer::Default;
    };
    let nodes: Vec<_> = rd
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("renderD"))
        .collect();
    if nodes.is_empty() {
        return Renderer::Software;
    }

    let mut saw_unknown = false;
    for n in &nodes {
        let dev = n.path().join("device");
        match gpu_kind(&dev) {
            Kind::Hardware => return Renderer::Default,
            Kind::NoAcceleration => {}
            Kind::Unknown => saw_unknown = true,
        }
    }
    if saw_unknown {
        Renderer::Default
    } else {
        Renderer::Software
    }
}

/// The real process environment and `/sys`.
pub fn choose_for_this_machine() -> Renderer {
    choose(|k| std::env::var(k).ok(), Path::new("/sys"))
}

enum Kind {
    Hardware,
    NoAcceleration,
    Unknown,
}

fn driver_name(dev: &Path) -> Option<String> {
    std::fs::read_link(dev.join("driver"))
        .ok()?
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
}

fn gpu_kind(dev: &Path) -> Kind {
    let driver = driver_name(dev).unwrap_or_default();
    if HARDWARE_DRIVERS.contains(&driver.as_str()) {
        return Kind::Hardware;
    }
    // A virtio GPU hangs off a virtio-pci device; the virtio child says
    // whether the host gave it 3D (feature bit 0, VIRTIO_GPU_F_VIRGL).
    if driver == "virtio-pci" || driver == "virtio_gpu" {
        let children = std::fs::read_dir(dev).into_iter().flatten().flatten();
        for c in children {
            if !c.file_name().to_string_lossy().starts_with("virtio") {
                continue;
            }
            if driver_name(&c.path()).as_deref() != Some("virtio_gpu") {
                continue;
            }
            return match std::fs::read_to_string(c.path().join("features")) {
                Ok(bits) if bits.starts_with('1') => Kind::Hardware,
                Ok(_) => Kind::NoAcceleration,
                Err(_) => Kind::Unknown,
            };
        }
        return Kind::Unknown;
    }
    // Display-only drivers that never have 3D.
    if matches!(
        driver.as_str(),
        "simpledrm" | "simple-framebuffer" | "efifb" | "bochs-drm" | "bochs" | "cirrus"
            | "cirrus-qemu" | "qxl" | "vboxvideo" | "hyperv_drm" | "udl" | "gm12u320" | "vkms"
    ) {
        return Kind::NoAcceleration;
    }
    Kind::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::path::PathBuf;

    fn sys(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("gust-sys-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("class/drm")).unwrap();
        std::fs::create_dir_all(d.join("drivers")).unwrap();
        d
    }

    /// renderD128 -> device, whose driver link names `driver`.
    fn node(s: &Path, name: &str, driver: &str) -> PathBuf {
        let dev = s.join(format!("devices/{name}"));
        std::fs::create_dir_all(&dev).unwrap();
        std::fs::create_dir_all(s.join("drivers").join(driver)).unwrap();
        symlink(s.join("drivers").join(driver), dev.join("driver")).unwrap();
        std::fs::create_dir_all(s.join("class/drm").join(name)).unwrap();
        symlink(&dev, s.join("class/drm").join(name).join("device")).unwrap();
        dev
    }

    fn virtio_child(s: &Path, dev: &Path, features: &str) {
        let v = dev.join("virtio2");
        std::fs::create_dir_all(&v).unwrap();
        std::fs::create_dir_all(s.join("drivers/virtio_gpu")).unwrap();
        symlink(s.join("drivers/virtio_gpu"), v.join("driver")).unwrap();
        std::fs::write(v.join("features"), features).unwrap();
    }

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn a_vm_with_a_2d_only_virtio_gpu_uses_the_raster_renderer() {
        // The exact shape of this development VM.
        let s = sys("virtio2d");
        let dev = node(&s, "renderD128", "virtio-pci");
        virtio_child(&s, &dev, "0100000000000000000000000000110010000000100000000000");
        assert_eq!(choose(no_env, &s), Renderer::Software);
    }

    #[test]
    fn a_vm_with_virgl_3d_keeps_the_gpu() {
        let s = sys("virgl");
        let dev = node(&s, "renderD128", "virtio-pci");
        virtio_child(&s, &dev, "1100000000000000000000000000110010000000100000000000");
        assert_eq!(choose(no_env, &s), Renderer::Default);
    }

    #[test]
    fn real_gpus_keep_the_gpu() {
        for drv in ["i915", "amdgpu", "nouveau", "nvidia-drm", "xe", "panfrost"] {
            let s = sys(&format!("hw-{drv}"));
            node(&s, "renderD128", drv);
            assert_eq!(choose(no_env, &s), Renderer::Default, "{drv}");
        }
    }

    #[test]
    fn a_hybrid_laptop_with_one_real_gpu_keeps_the_gpu() {
        let s = sys("hybrid");
        node(&s, "renderD128", "simpledrm");
        node(&s, "renderD129", "amdgpu");
        assert_eq!(choose(no_env, &s), Renderer::Default);
    }

    #[test]
    fn no_render_node_at_all_uses_the_raster_renderer() {
        let s = sys("none");
        assert_eq!(choose(no_env, &s), Renderer::Software);
    }

    #[test]
    fn display_only_drivers_use_the_raster_renderer() {
        let s = sys("qxl");
        node(&s, "renderD128", "qxl");
        assert_eq!(choose(no_env, &s), Renderer::Software);
    }

    #[test]
    fn an_unknown_driver_is_never_guessed_at() {
        let s = sys("unknown");
        node(&s, "renderD128", "some-future-gpu");
        assert_eq!(choose(no_env, &s), Renderer::Default);
    }

    #[test]
    fn the_user_decides_first() {
        let s = sys("user");
        node(&s, "renderD128", "amdgpu");
        let sw = |k: &str| (k == "GUST_RENDERER").then(|| "software".to_string());
        assert_eq!(choose(sw, &s), Renderer::Software);

        let s2 = sys("user2");
        let dev = node(&s2, "renderD128", "virtio-pci");
        virtio_child(&s2, &dev, "0");
        let gpu = |k: &str| (k == "GUST_RENDERER").then(|| "gpu".to_string());
        assert_eq!(choose(gpu, &s2), Renderer::Default);
        // A Qt-level choice the user already made is respected as well.
        let qt = |k: &str| (k == "QT_QUICK_BACKEND").then(|| "opengl".to_string());
        assert_eq!(choose(qt, &s2), Renderer::Default);
    }
}
