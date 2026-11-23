//! Scaling to the analysis size on the GPU, before the picture is downloaded.
//!
//! A hardware decoder leaves its pictures in GPU memory. Bringing every full-size
//! picture into system memory and shrinking it there costs far more than shrinking
//! it first and downloading the small result, so each hardware family has a matching
//! scaler that runs between the decoder and the CPU's `videoscale`/`videoconvert`.
//!
//! Scalers are always optional: one that is not installed is never planned, one that
//! fails on a camera is given up for that camera (the same decoder carries on with
//! CPU scaling), and a picture that needs no scaling never goes through one
//! (`vapostproc` crashed in every non-scaling mode under the i965 driver).
//!
//! Families without a scaler here scale on the CPU, on purpose:
//! Intel Quick Sync (`qsvvpp` is newer than the oldest GStreamer this runs on and
//! its memory is not ours to download), the legacy `vaapi*` and `msdk*` plugins,
//! and V4L2 (`v4l2convert` works on whatever buffers the board's driver makes).

/// The status never says more than this: Diagnostics clips decoder text there.
pub const MAX_STATUS_CHARS: usize = 96;

/// The status suffix; a parser may look for exactly this.
const SCALED_ON: &str = " · scaled down on the ";

/// A hardware family's scaler.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scaler {
    /// `vah264dec` / `vah265dec` → `vapostproc`
    Va,
    /// `nvh264dec` / `nvh265dec` → `cudaconvertscale` → `cudadownload`
    Cuda,
    /// `d3d11h264dec` / `d3d11h265dec` → `d3d11convert` → `d3d11download`
    D3d11,
    /// `d3d12h264dec` / `d3d12h265dec` → `d3d12convert` → `d3d12download`
    D3d12,
}

impl Scaler {
    pub const ALL: [Self; 4] = [Self::Va, Self::Cuda, Self::D3d11, Self::D3d12];

    /// The element that scales.
    pub fn element(self) -> &'static str {
        match self {
            Self::Va => "vapostproc",
            Self::Cuda => "cudaconvertscale",
            Self::D3d11 => "d3d11convert",
            Self::D3d12 => "d3d12convert",
        }
    }

    /// The element that brings the small picture into system memory, for the
    /// families whose scaler leaves it in GPU memory (`vapostproc` outputs
    /// system memory itself).
    pub fn download(self) -> Option<&'static str> {
        match self {
            Self::Va => None,
            Self::Cuda => Some("cudadownload"),
            Self::D3d11 => Some("d3d11download"),
            Self::D3d12 => Some("d3d12download"),
        }
    }

    /// The scaler that goes with a decoder element. The legacy `vaapi*`,
    /// `msdk*`, `qsv*` and `v4l2*` decoders have none.
    pub fn of_decoder(decoder: &str) -> Option<Self> {
        match decoder {
            "vah264dec" | "vah265dec" => Some(Self::Va),
            "nvh264dec" | "nvh265dec" => Some(Self::Cuda),
            "d3d11h264dec" | "d3d11h265dec" => Some(Self::D3d11),
            "d3d12h264dec" | "d3d12h265dec" => Some(Self::D3d12),
            _ => None,
        }
    }

    /// The GPU memory its scaler works on, where it must be said.
    fn memory(self) -> Option<&'static str> {
        match self {
            Self::Va => None,
            Self::Cuda => Some("CUDAMemory"),
            Self::D3d11 => Some("D3D11Memory"),
            Self::D3d12 => Some("D3D12Memory"),
        }
    }

    /// Every element it needs here.
    fn elements(self) -> impl Iterator<Item = &'static str> {
        std::iter::once(self.element()).chain(self.download())
    }

    /// The pipeline text between the decoder and `videoscale`: scale to
    /// `width`×`height`, then (where needed) download. The picture stays in the
    /// decoder's format; `videoconvert` makes the analysis format at this size.
    pub fn segment(self, width: u32, height: u32) -> String {
        let caps = match self.memory() {
            Some(memory) => format!("video/x-raw(memory:{memory}),width={width},height={height}"),
            None => format!("video/x-raw,width={width},height={height}"),
        };
        let scale = format!("{} name=scale ! {caps}", self.element());
        match self.download() {
            Some(download) => format!("{scale} ! {download} name=download"),
            None => scale,
        }
    }
}

/// A scaler planned for a camera, and the size it scales to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuScale {
    pub scaler: Scaler,
    pub width: u32,
    pub height: u32,
}

impl GpuScale {
    pub fn segment(self) -> String {
        self.scaler.segment(self.width, self.height)
    }
}

/// The scaler for `decoder`, if it has one, GStreamer here has all its
/// elements, and it has not let this camera down.
pub fn scaler_for(
    decoder: &str,
    failed: &[Scaler],
    installed: &dyn Fn(&str) -> bool,
) -> Option<Scaler> {
    let scaler = Scaler::of_decoder(decoder)?;
    (!failed.contains(&scaler) && scaler.elements().all(installed)).then_some(scaler)
}

/// The size the GPU scales a `source`-sized picture to, when it really scales:
/// `target_width` wide and `target_height` high, or (`None`) the source's shape
/// at that width, to even sizes (the formats the GPU makes are subsampled).
/// A picture not larger than the target in both directions, or a source whose
/// size is unknown, gets none: CPU scaling.
pub fn gpu_size(
    source: (u32, u32),
    target_width: u32,
    target_height: Option<u32>,
) -> Option<(u32, u32)> {
    let (source_width, source_height) = source;
    if source_width == 0 || source_height == 0 || target_width == 0 {
        return None;
    }
    let height = match target_height {
        Some(height) => height,
        None => {
            let exact = (u64::from(target_width) * u64::from(source_height)
                + u64::from(source_width) / 2)
                / u64::from(source_width);
            // To the nearest even number, at least 2.
            u32::try_from(exact.div_ceil(2) * 2).ok()?.max(2)
        }
    };
    (source_width > target_width && source_height > height).then_some((target_width, height))
}

/// What to scale on the GPU: [`scaler_for`] and [`gpu_size`] together.
pub fn plan(
    decoder: &str,
    source: (u32, u32),
    target_width: u32,
    target_height: Option<u32>,
    failed: &[Scaler],
    installed: &dyn Fn(&str) -> bool,
) -> Option<GpuScale> {
    let scaler = scaler_for(decoder, failed, installed)?;
    let (width, height) = gpu_size(source, target_width, target_height)?;
    Some(GpuScale {
        scaler,
        width,
        height,
    })
}

/// The scalers whose elements are all here, for `--check`.
pub fn available(installed: &dyn Fn(&str) -> bool) -> Vec<&'static str> {
    Scaler::ALL
        .into_iter()
        .filter(|scaler| scaler.elements().all(installed))
        .map(Scaler::element)
        .collect()
}

/// The status text: who decodes, then who scaled. `decoder` is the decoder's
/// text ([`crate::decoding::status_text`]); where a long note would pass
/// [`MAX_STATUS_CHARS`] it is cut short (with "…"), never the suffix.
pub fn status_line(decoder: &str, on_gpu: bool) -> String {
    let suffix = format!("{SCALED_ON}{}", if on_gpu { "GPU" } else { "CPU" });
    let room = MAX_STATUS_CHARS.saturating_sub(suffix.chars().count());
    if decoder.chars().count() <= room {
        return format!("{decoder}{suffix}");
    }
    let kept: String = decoder.chars().take(room.saturating_sub(1)).collect();
    format!("{}…{suffix}", kept.trim_end())
}

/// Why this camera scales on the CPU although its decoder has a GPU scaler.
pub fn given_up_note(scaler: Scaler, while_running: bool) -> String {
    if while_running {
        format!(
            "Scaling on the CPU: {} kept failing while running",
            scaler.element()
        )
    } else {
        format!(
            "Scaling on the CPU: {} could not scale this video",
            scaler.element()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(_: &str) -> bool {
        true
    }

    #[test]
    fn each_family_scales_with_its_own_element() {
        let of = |decoder| scaler_for(decoder, &[], &all);
        assert_eq!(of("vah264dec"), Some(Scaler::Va));
        assert_eq!(of("vah265dec"), Some(Scaler::Va));
        assert_eq!(of("nvh264dec"), Some(Scaler::Cuda));
        assert_eq!(of("nvh265dec"), Some(Scaler::Cuda));
        assert_eq!(of("d3d11h264dec"), Some(Scaler::D3d11));
        assert_eq!(of("d3d12h265dec"), Some(Scaler::D3d12));
        for none in [
            "avdec_h264",
            "vaapih264dec",
            "qsvh264dec",
            "msdkh264dec",
            "v4l2slh264dec",
            "v4l2h265dec",
            "somedec",
        ] {
            assert_eq!(of(none), None, "{none} scales on the CPU");
        }
    }

    #[test]
    fn a_scaler_that_is_not_installed_or_has_failed_is_not_planned() {
        assert_eq!(
            scaler_for("vah264dec", &[], &|e| e != "vapostproc"),
            None,
            "this machine: vapostproc is not registered"
        );
        // Both of a two-element scaler's elements are needed.
        assert_eq!(scaler_for("nvh264dec", &[], &|e| e != "cudadownload"), None);
        assert_eq!(
            scaler_for("nvh264dec", &[Scaler::Va], &all),
            Some(Scaler::Cuda),
            "another family's failure is not this one's"
        );
        assert_eq!(scaler_for("vah264dec", &[Scaler::Va], &all), None);
    }

    #[test]
    fn the_gpu_only_scales_what_gets_smaller() {
        // Motion's fixed 320x180.
        assert_eq!(gpu_size((1920, 1080), 320, Some(180)), Some((320, 180)));
        assert_eq!(gpu_size((640, 360), 320, Some(180)), Some((320, 180)));
        assert_eq!(gpu_size((321, 181), 320, Some(180)), Some((320, 180)));
        for none in [
            (320, 180),
            (320, 240),
            (640, 180),
            (160, 90),
            (0, 0),
            (1920, 0),
        ] {
            assert_eq!(gpu_size(none, 320, Some(180)), None, "{none:?}");
        }
        // Detector's width, shape kept, even height.
        assert_eq!(gpu_size((1920, 1080), 416, None), Some((416, 234)));
        assert_eq!(gpu_size((2560, 1440), 416, None), Some((416, 234)));
        assert_eq!(gpu_size((1280, 960), 416, None), Some((416, 312)));
        assert_eq!(gpu_size((704, 576), 416, None), Some((416, 340)));
        assert_eq!(gpu_size((416, 234), 416, None), None);
        assert_eq!(gpu_size((352, 288), 416, None), None);
        assert_eq!(gpu_size((1920, 1080), 0, None), None);
        // Odd shapes still come out even and at least 2.
        let (_, height) = gpu_size((4000, 3), 416, None).unwrap_or((0, 0));
        assert_eq!(height % 2, 0);
    }

    #[test]
    fn plan_needs_everything() {
        let plan = |decoder, source| plan(decoder, source, 416, None, &[], &all);
        assert_eq!(
            plan("vah264dec", (1920, 1080)),
            Some(GpuScale {
                scaler: Scaler::Va,
                width: 416,
                height: 234
            })
        );
        assert_eq!(plan("vah264dec", (416, 234)), None);
        assert_eq!(plan("avdec_h264", (1920, 1080)), None);
    }

    #[test]
    fn the_pipeline_text_per_family() {
        assert_eq!(
            Scaler::Va.segment(320, 180),
            "vapostproc name=scale ! video/x-raw,width=320,height=180"
        );
        assert_eq!(
            Scaler::Cuda.segment(416, 234),
            "cudaconvertscale name=scale ! video/x-raw(memory:CUDAMemory),width=416,height=234 ! cudadownload name=download"
        );
        assert_eq!(
            Scaler::D3d11.segment(320, 180),
            "d3d11convert name=scale ! video/x-raw(memory:D3D11Memory),width=320,height=180 ! d3d11download name=download"
        );
        assert_eq!(
            Scaler::D3d12.segment(320, 180),
            "d3d12convert name=scale ! video/x-raw(memory:D3D12Memory),width=320,height=180 ! d3d12download name=download"
        );
    }

    #[test]
    fn available_lists_complete_scalers_only() {
        assert_eq!(available(&all).len(), 4);
        assert_eq!(available(&|e| e != "d3d11download").len(), 3);
        assert_eq!(available(&|_| false), Vec::<&str>::new());
        assert_eq!(available(&|e| e == "vapostproc"), vec!["vapostproc"]);
    }

    #[test]
    fn the_status_names_who_scaled() {
        assert_eq!(
            status_line("VA-API (Intel, AMD)", true),
            "VA-API (Intel, AMD) · scaled down on the GPU"
        );
        assert_eq!(
            status_line("CPU (software)", false),
            "CPU (software) · scaled down on the CPU"
        );
        assert_eq!(
            status_line("NeuroWatch's pictures (no decoding here)", false),
            "NeuroWatch's pictures (no decoding here) · scaled down on the CPU"
        );
        assert_eq!(
            status_line("CPU (software): VA-API kept failing while running", false),
            "CPU (software): VA-API kept failing while running · scaled down on the CPU"
        );
    }

    #[test]
    fn a_long_decoder_text_is_cut_to_exactly_96_characters_with_the_suffix_whole() {
        // The wire contract the desktop and portal parse: the decoder part is
        // cut first, ends with "…", the suffix is never touched.
        assert_eq!(MAX_STATUS_CHARS, 96);
        let long = "x".repeat(120);
        assert_eq!(
            status_line(&long, true),
            format!("{}… · scaled down on the GPU", "x".repeat(70))
        );
        assert_eq!(
            status_line(&long, false),
            format!("{}… · scaled down on the CPU", "x".repeat(70))
        );
        assert_eq!(status_line(&long, true).chars().count(), 96);
        // Exactly at the limit is not cut.
        let fits = "y".repeat(71);
        assert_eq!(
            status_line(&fits, true),
            format!("{fits} · scaled down on the GPU")
        );
    }

    #[test]
    fn the_status_stays_under_what_diagnostics_shows() {
        // The longest note decoding can make: a leaking VA plugin.
        let longest = crate::decoding::status_text(
            "avdec_h264",
            Some("on the CPU: GStreamer 1.24.2's VA-API leaks memory (fixed in 1.24.6)"),
        );
        for on_gpu in [true, false] {
            let line = status_line(&longest, on_gpu);
            assert!(line.chars().count() <= MAX_STATUS_CHARS, "{line}");
            let suffix = if on_gpu {
                " · scaled down on the GPU"
            } else {
                " · scaled down on the CPU"
            };
            assert!(line.ends_with(suffix), "the suffix is never cut: {line}");
            assert!(line.starts_with("CPU (software): GStreamer"));
            assert!(line.contains('…'));
        }
        for shown in [
            "VA-API (Intel, AMD)",
            "V4L2 (ARM boards)",
            "NVIDIA NVDEC",
            "Intel Quick Sync",
        ] {
            let note = format!("{shown}: Direct3D 12 could not decode this video");
            assert!(status_line(&note, true).chars().count() <= MAX_STATUS_CHARS);
        }
    }

    #[test]
    fn the_notes_say_what_happened() {
        assert_eq!(
            given_up_note(Scaler::Va, false),
            "Scaling on the CPU: vapostproc could not scale this video"
        );
        assert_eq!(
            given_up_note(Scaler::Cuda, true),
            "Scaling on the CPU: cudaconvertscale kept failing while running"
        );
    }
}
