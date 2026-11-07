# Motion detection efficiency research

Research date: 2026-10-31. Scope: the current checkout's detector, media pipeline,
CPU instructions, memory use, and scaling across cameras. Production code was not
changed. Recommendations below distinguish measured findings from proposals.

## Recommendation

Updated 2026-11-01: detection quality is the primary requirement; CPU-only operation
is required and GPU acceleration is optional. The earlier speed-first suggestion
to retain the current background model is superseded by
[`motion-analysis-quality.md`](motion-analysis-quality.md). Evaluate established
foreground segmentation implementations before selecting a replacement, and
optimize the selected algorithm after it passes accuracy tests. The measurements
below describe the existing detector, not evidence that it is state of the art.

For installations that decode video independently, avoiding duplicate decoding or
decoding a smaller camera stream may save more CPU than accelerating the detector.

The most convincing measured detector improvement was a simple condition: do not
scan include zones when there are no changed blocks. A temporary prototype reduced
quiet-scene analysis time by about 4.2 times with 32 include zones in the recorded
run. That is a detector result, not a daemon throughput claim.

## Current work and likely costs

The compressed-video path in `src/worker.rs` is:

```text
H.264/H.265 -> decodebin -> drop to <=5 fps -> videoscale -> videoconvert
           -> 320x180 GRAY8 -> copy to Vec -> Detector::observe
```

Consequently, the 5 fps limit saves downstream work but does not itself reduce the
number of frames entering the decoder. GStreamer's `videorate` operates on raw
frames and limits output by dropping them. Arbitrarily dropping compressed delta
frames before decoding would break prediction dependencies. A source stream
configured at a lower frame rate avoids that work earlier in the pipeline.
[GStreamer videorate](https://gstreamer.freedesktop.org/documentation/videorate/index.html)

The local shared-picture path already reuses desktop pictures when available. It
still copies the supplied RGB/NV12/gray image into a GStreamer buffer, converts and
scales it, then copies the resulting grayscale picture into a new allocation.
Low-CPU mode already requests compressed keyframes only and sacrifices short-event
coverage. These existing paths should be measured separately.

Inside `src/detector.rs`, each normal observation:

1. Allocates a 14,400-byte image and averages each 2x2 input neighborhood.
2. Allocates 14,400 change flags and compares against a 16-bit fixed-point background.
3. Counts changes across the entire image to detect global lighting changes.
4. Walks every pixel again, recomputing watched block areas and updating the background.
5. Collects changed blocks and constructs their bounding box.
6. Scans up to 14,400 pixels for each include zone to identify affected zones.

The last step is O(pixels * include zones) in the worst case, including quiet
frames. A block's watched area and the number of watched blocks are invariant until
the zone configuration changes, but are currently recalculated each frame.

The worker also constructs debug drawing objects every analyzed frame, including
quiet frames. JSON construction and transport can matter at camera counts where
the pixel detector is already inexpensive. Visibility/subscription information
would be needed to safely stop sending debug content; a closed dashboard alone is
not currently a reliable signal in this code.

## Measurements and their limits

Machine: Intel Core i7-8650U, x86-64, AVX2 and POPCNT available; Rust 1.94.1.
Detector source SHA-256 at measurement time:
`b2b704198e2ab41771bb5e473b855beecbc6218c73f9e3e135a582f6da44971c`.
The standalone harness compiles the actual detector source with equivalent
settings structs using `rustc -C opt-level=3 -C codegen-units=1`. It excludes
GStreamer, decoding, configuration deserialization, drawings, and transport.

The recorded comparison uses Linux thread CPU time, CPU affinity to logical CPU 0,
five interleaved trials, 128 warmup frames, and 1,000 timed observations per trial.
Inputs are prebuilt quiet frames or a moving bright rectangle. Include zones in
the timing cases are vertical strips covering the image. Setup is outside timing;
allocations in `observe` and sample destruction remain inside timing.

| Quiet-scene configuration | Original median, microseconds/frame | Skip zone scans when blocks are empty | Relative speed |
|---|---:|---:|---:|
| No zones | 190.7 | 178.2 | Small difference; do not infer a benefit |
| 8 include zones | 495.6 | 246.6 | About 2.0x |
| 32 include zones | 1,266.8 | 305.0 | About 4.2x |

The shortcut changes only zone enumeration. An empty block list means no pixel can
pass the existing `active_blocks[...]` predicate, so the resulting zone list must
already be empty. It must not bypass background learning or event timing.

A second temporary variant makes the two background-update divisions explicit:

```rust
let delta = target - current;
let step = if changed[i] { delta / 256 } else { delta / 32 };
self.background[i] = (current + step) as u16;
```

Assembly inspection found an `idivl` in the original per-pixel update loop and no
integer divide in the rewritten harness. This removes an avoidable instruction,
but overall timings were mixed. Do not claim a reliable speedup from this rewrite
alone. Separating the background-update loop from masked block accumulation may
make the update easier to vectorize and should be benchmarked next.

The combined prototype matched all observable sample fields on 12,800 replayed
samples: active state, exact score bits, bounding box, zone IDs, and changed block
IDs. Replays covered all ten sensitivities, 0/1/8/32 zone configurations, overlapping
include/exclude masks, motion, isolated noise, brightness changes, and timing.
This is useful evidence, not an exhaustive equivalence proof or camera-quality
evaluation. Explicit SIMD kernels have not been implemented or benchmarked.

Measurements varied substantially between trials and runs on this laptop.
Thread CPU time reduces scheduling interference but does not normalize clock
frequency, thermal effects, or contention. Moving-scene results did not show a
consistent improvement. Initial separate generic/native builds did not establish
a benefit from `target-cpu=native`; they were not a controlled ISA comparison.
Repeat on deployment hardware before capacity planning. Full paired results,
including moving cases and min/max timings, are in
[`research/motion-detector-results.csv`](research/motion-detector-results.csv).

Reproduce on Linux with Python 3 and rustc, without the Cargo/GStreamer dependencies:

```sh
python3 docs/research/run_motion_detector_benchmark.py --iterations 1000
python3 docs/research/run_motion_detector_benchmark.py --iterations 1000 --native
```

The runner copies prototype variants into a temporary directory and leaves
production sources untouched. Its template uses Linux's thread CPU clock and is
intended for this project's 64-bit targets.

## CPU instruction options

SIMD applies the same operation to several pixels in one instruction. Good
candidates here are 2x2 smoothing, brightness differences, threshold comparisons,
change counting, and background updates. Rust provides stable architecture
intrinsics and feature detection through `std::arch`.
[Rust architecture intrinsics](https://doc.rust-lang.org/std/arch/)

| Target | Useful operations | Recommended role |
|---|---|---|
| x86-64 SSE2, 128 bits | Packed subtraction, comparison, widening, shifts | Portable x86-64 baseline; compare with compiler output |
| x86-64 AVX2, 256 bits | Process up to 32 byte differences or 8 i32 updates per vector | First explicit x86 optimization to investigate |
| POPCNT | Count set bits in change/block masks | Useful with compact bitsets; detect separately if a kernel requires it |
| AArch64 NEON, 128 bits | Unsigned absolute difference, comparisons, widening, shifts | Main ARM implementation, including ARM64 Raspberry Pi deployments |
| AVX-512BW / ARM SVE | Wider integer vectors | Later, only if representative hardware measurements justify maintenance |

AVX2 has unsigned saturated byte subtraction. Two such subtractions ORed together
compute unsigned absolute differences without 8-bit signed overflow. Compare
those differences against the threshold, then reduce the resulting flags.
[Rust AVX2 saturated subtraction](https://doc.rust-lang.org/core/arch/x86_64/fn._mm256_subs_epu8.html)

NEON offers unsigned absolute difference directly through `vabdq_u8`; Arm's Rust
SIMD tutorial demonstrates its relationship to the `uabd` instruction. This is a
good match for pixel comparison.
[Arm Rust SIMD tutorial](https://learn.arm.com/learning-paths/cross-platform/simd-on-rust/simd-on-rust-part2/)

Byte width is not a speedup forecast. The background update needs wider signed
intermediates, reductions cost instructions, and allocations, zones, and decoding
remain outside the accelerated kernel. If the detector accounts for 10% of total
CPU time, making it four times faster improves overall throughput by only about
8%: `1 / (0.90 + 0.10 / 4)`.

The existing generic build already emits packed SSE instructions for the pixel
difference loop. LLVM can vectorize suitable loops automatically. Rewrite loops
for contiguous access and simple control flow, inspect assembly, then compare
explicit intrinsics with that baseline.
[LLVM vectorization documentation](https://llvm.org/docs/Vectorizers.html)

Dispatch once when creating a detector: AVX2 when available, otherwise a baseline;
use an AArch64 NEON path in ARM64 builds. Keep a reference implementation for
differential verification. If an x86 kernel also relies on POPCNT, feature dispatch
must account for that separately.

Do not globally enable `target-cpu=native` for distributed packages: the resulting
binary may require instructions absent on customer machines. It is useful for
local experiments. It also does not rebuild separately compiled FFmpeg/GStreamer
shared libraries. Stable `std::arch` is the most direct initial option;
`std::simd` is still documented as experimental/nightly at the research date.
[Rust CPU options](https://doc.rust-lang.org/rustc/codegen-options/index.html#target-cpu),
[portable SIMD status](https://doc.rust-lang.org/std/simd/index.html)

### Exactness requirements

The current background stores brightness multiplied by 256. The signed difference
can exceed i16 range, so a straightforward SIMD implementation should use i32
intermediates. Preserve strict `difference > threshold` behavior and the special
whole-image lighting reset before committing normal background updates.

Rust signed division truncates toward zero, whereas a signed arithmetic right
shift rounds negative values downward. Replacing `/ 32` or `/ 256` with a plain
shift changes background learning. Use constant divisions that the compiler can
lower correctly, or explicitly correct negative rounding. Also widen and sum all
four smoothing samples before dividing by four: repeated rounded SIMD averages
need not match the existing result.
[Rust arithmetic semantics](https://doc.rust-lang.org/reference/expressions/operator-expr.html)

## Structural improvements before more complex algorithms

| Change | Expected reason to help | Status / caveat |
|---|---|---|
| Skip zones when no blocks changed | Removes unnecessary zone scans in quiet scenes | Measured; exact for existing predicate |
| Cache watched area per block and watched block count | Removes invariant mask work | Proposal; rebuild on settings changes |
| Reuse grayscale/change buffers | Removes repeated heap allocation and initialization | Proposal; preserve sample ownership |
| Process mapped, strided grayscale frames directly | Removes the worker's 57,600-byte copy | Proposal; respect mapping lifetime and stride |
| Separate vector-friendly pixel kernels from block reductions | Makes contiguous SIMD work easier | Proposal; measure extra passes versus fusion |
| Use bitsets for changes and zone masks | Improves memory footprint and intersection cost | Proposal; most useful with many zones |
| Limit debug generation to subscribed demand or a lower rate | Reduces allocations, serialization, and messages | Requires protocol/behavior review; preserve overlay expiry |
| Tune decoder threads per stream | Avoids excessive thread counts across cameras | Profile at realistic concurrency |

For bitsets, one 160x90 zone mask needs 225 u64 words, or 1,800 bytes, instead of
14,400 bytes of boolean entries. At 32 include zones that is 57,600 versus 460,800
bytes for zone masks alone. Test intersection of zone, watched-change, and active
block pixel masks; using only block membership would introduce false zone hits at
polygon edges. A per-pixel u32 membership mask is another option because the
configuration allows at most 32 zones. Neither layout has been benchmarked here.

Do not parallelize the tiny image across threads initially. Cameras already have
individual worker threads, and their decoders can create additional threads.
`avdec_h264` exposes `max-threads` with automatic selection by default. Compare
small fixed decoder thread counts with automatic selection under many-camera
load; one stream's maximum decode speed may not maximize overall throughput.
[GStreamer avdec_h264](https://gstreamer.freedesktop.org/documentation/libav/avdec_h264.html)

## Reducing media work

1. **Prefer an appropriate camera substream or existing shared decode.** Benchmark
   the full cost, including copying a larger desktop Main picture versus decoding
   a small compressed Sub stream. Sharing is already implemented; its presence
   alone does not establish that it is cheaper for every resolution/format.
2. **Use luma directly when practical.** For NV12/I420 the Y plane provides
   brightness without an RGB conversion. Handle stride, color range, format, and
   bit depth explicitly, and recalibrate if grayscale numerical values differ.
3. **Consider producing the final 160x90 analysis image once.** Current conversion
   produces 320x180 and Rust downsamples again. A single resize can reduce traffic,
   but interpolation/rounding changes can affect small objects and sensitivity.
   Keep the current analysis grid and compare event behavior before adoption.
4. **Benchmark combined conversion/scaling.** `videoconvertscale` supports both
   operations, but do not assume replacing the two named elements is faster:
   negotiation and existing fast paths matter.
   [GStreamer videoconvertscale](https://gstreamer.freedesktop.org/documentation/videoconvertscale/videoconvertscale.html)
5. **Use hardware decoding with hardware scaling when beneficial.** The existing
   flag permits hardware decoder selection; it does not establish a GPU scaling
   path. For VA-API, investigate keeping surfaces on the device through
   `vapostproc`, scaling down before transferring data to the CPU. Capabilities
   depend on the driver; measure transfers, device contention, and CPU together.
   [VA decoding](https://gstreamer.freedesktop.org/documentation/va/vah264dec.html),
   [VA postprocessing](https://gstreamer.freedesktop.org/documentation/va/vapostproc.html)

GStreamer conversion/scaling already uses ORC SIMD acceleration in supported
paths. The inspected sibling media build enables ORC and packages it, and FFmpeg
has architecture-specific SIMD implementations. Much of the media stack already
uses CPU instructions; profiling should determine which remaining Rust work is
worth optimizing.
[GStreamer ORC announcement](https://discourse.gstreamer.org/t/orc-0-4-36-release/883),
[FFmpeg H.264 x86 initialization](https://github.com/FFmpeg/FFmpeg/blob/master/libavcodec/x86/h264dsp_init.c)

## More experimental alternatives

**Compressed motion vectors:** FFmpeg can export vectors for supporting codecs.
This is worth investigating as a coarse signal, but the API does not promise
decoding-free motion detection, support for every codec, or vectors from every
hardware decoder. Prediction vectors are not object motion labels. The current
GStreamer appsink exposes pixels, so exploiting them needs additional integration.
Prototype against actual H.264 and H.265 camera streams before selecting this
architecture.
[FFmpeg export_mvs](https://www.ffmpeg.org/ffmpeg-all.html)

**Adaptive frame rates / coarse screening:** these save detector work, and save
decode work only when the source or decoder actually does less. They can miss
brief or small movements. Background adaptation currently advances per observed
frame; changing sampling rate changes its effective learning time even though
minimum duration and cooldown use timestamps. A time-based learning rule and a
recorded-video accuracy evaluation would be needed.

**GPU compute, larger background models, and neural models:** do not start here
for throughput optimization of a 160x90 detector. They add transfer, launch,
integration, or computational costs. Revisit when existing GPU-resident images
or measured accuracy requirements justify them. This is an engineering judgment,
not a measured comparison with those implementations.

## Implementation and validation order

1. Measure release-mode CPU samples and allocation rates for decode, conversion,
   copying, detector, drawings/JSON, and IPC. Compare compressed and shared input,
   H.264/H.265, Main/Sub, 0/8/32 zones, and realistic camera concurrency. GStreamer
   offers resource and latency tracers; latency alone is not CPU cost.
   [GStreamer tracing](https://gstreamer.freedesktop.org/documentation/additional/design/tracing.html)
2. Apply the empty-block zone shortcut, cached geometry, buffer reuse, and a
   separate background update experiment. Compare each change independently.
3. Add runtime AVX2 and AArch64 NEON kernels only for measured detector hot loops.
   Verify exact outputs against the reference, including threshold boundaries,
   darkening, lighting resets, polygon edges, minimum duration, and cooldown.
4. Optimize decoding, luma access, scaling, and sharing according to measured CPU
   distribution. Record dropped frames, event latency, memory, and energy as well
   as raw throughput.
5. Evaluate changes to sampling or motion logic separately using annotated camera
   recordings. Report missed events and false alarms; computational savings alone
   do not establish a better detector.

No whole-daemon CPU profile, live-camera accuracy comparison, power measurement,
ARM measurement, or explicit SIMD speedup is claimed by this research.
