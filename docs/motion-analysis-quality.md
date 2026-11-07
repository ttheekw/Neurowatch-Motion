# Motion analysis: algorithm selection and quality requirements

Updated 2026-11-01. User requirements: established, high-quality motion analysis;
CPU-only operation required, GPU optional. This document revises the earlier
efficiency-only recommendation. The first CPU comparison is now implemented and
measured in [`research/cpu-motion-comparison/README.md`](research/cpu-motion-comparison/README.md).
No replacement detector has been integrated into production.

## Decision

Use published algorithms and their reference implementations as the starting
point. The current single-background brightness detector is a legacy comparison,
not the quality target. Selecting an algorithm requires both held-out accuracy and
measured CPU/memory cost. SIMD follows that selection.

For a CPU implementation, evaluate upstream **SuBSENSE and PAWCS** against OpenCV
MOG2/KNN and the existing detector. SuBSENSE is the first integration candidate
because its reference implementation supports CPU processing, texture descriptors,
and per-pixel adaptive sensitivity. PAWCS is the comparison for a more persistent
word-consensus background model. These are established classical methods, not a
claim about the globally best model in 2026.
[SuBSENSE reference implementation](https://raw.githubusercontent.com/plstcharles/litiv/master/modules/video/include/litiv/video/BackgroundSubtractorSuBSENSE.hpp),
[PAWCS reference implementation](https://raw.githubusercontent.com/plstcharles/litiv/master/modules/video/include/litiv/video/BackgroundSubtractorPAWCS.hpp)

For optional learned analysis, evaluate **SeqFeedNet (WACV 2026)** first because it
targets background subtraction on previously unseen videos. Keep **GMOS/GMOS-S
(2026)** as a research candidate for camera-independent object motion, rather than
assuming a general segmentation model can determine motion on its own. Actual
deployment remains contingent on weights, initialization, causal operation,
runtime compatibility, and tests on the supported hardware.
[SeqFeedNet paper](https://openaccess.thecvf.com/content/WACV2026/papers/Huang_SeqFeedNet_Sequential_Feature_Feedback_Network_for_Background_Subtraction_WACV_2026_paper.pdf),
[GMOS paper](https://arxiv.org/html/2605.30352v1)

## Why optimizing the existing algorithm is insufficient

Code inspection of `src/detector.rs` shows one brightness value per background
pixel, one global sensitivity threshold, and a global reset whenever more than
60% of the image changes. There is no multimodal background distribution, texture
descriptor, explicit shadow model, connected-component analysis, or camera-motion
estimation in the detector. The worker suppresses analysis when told the camera
is moving, but that does not compensate for unannounced jitter or estimate
independent object motion.

Those design choices leave plausible failure cases: repeated foliage movement,
shadows, brightness flicker, a large foreground object triggering a scene reset,
small objects lost at 160x90, and objects absorbed into the background. These are
code-based hypotheses to test, not measured error rates.

The previous 12,800-sample experiment verified that two speed optimizations
preserved the existing output. It did not establish whether those outputs were
correct relative to real motion or benchmark ground truth.

## Candidate evidence and practical implications

| Candidate | Role | What is established | What remains to verify here |
|---|---|---|---|
| Existing detector | Legacy reference | Minimal background model; current event behavior | Actual false alarms and missed events |
| OpenCV MOG2 / KNN | Reproducible baseline | Maintained implementations of established background subtraction | Accuracy, shadow treatment, speed at target resolution |
| SuBSENSE | First CPU integration candidate | Sample-based color/texture model with local adaptation; reference CPU source | Best settings, resolution, memory, alarm quality, portability |
| PAWCS | CPU quality comparison | Adaptive local/global word-consensus model; reference CPU source | Whether extra complexity improves the target scenes |
| SeqFeedNet, WACV 2026 | First learned candidate | Published unseen-video BGS evaluation and public model implementation | Causal startup, deployment checkpoint, CPU cost, acceleration/export |
| GMOS / GMOS-S, 2026 | Research comparison | RGB moving-object segmentation with camera-motion reasoning and online variant | Public deployable artifacts, license/weights, performance on target hardware |

OpenCV's tutorial supplies working MOG2/KNN examples, making them useful sanity
checks. A replacement should beat a credible maintained baseline, not merely the
existing custom detector.
[OpenCV background subtraction](https://docs.opencv.org/4.13.0/d1/dc5/tutorial_background_subtraction.html)

The CDnet benchmark explicitly warns against comparing supervised and
unsupervised leaderboard entries without examining training and evaluation.
Scene-specific training is particularly unsuitable for a recorder that must work
on newly added cameras. Do not select the largest headline F-score without
checking whether test-camera labels were used.
[CDnet results and evaluation warning](https://changedetection.net/results2014/)

SeqFeedNet's official repository reports overall F-measure 0.8422 under its
unseen-video evaluation and identifies an RTX 3090 as the GPU used for speed
measurements. These are author-reported results, not measurements of NeuroWatch or
CPU throughput. Different folds show substantial category variation.
[SeqFeedNet official repository](https://github.com/tw-yshuang/SeqFeedNet)

Source inspection is important: the SeqFeedNet forward function consumes an empty
background image, recurrent features, and background-only images alongside the
current RGB frame. The supplied data loader reads prepared empty/recent background
assets. We need a causal way to initialize those inputs from a live stream; we
cannot assume a checkpoint and one grayscale frame are sufficient.
[SeqFeedNet model source](https://raw.githubusercontent.com/tw-yshuang/SeqFeedNet/main/models/SeqFeedNet.py),
[SeqFeedNet dataset source](https://raw.githubusercontent.com/tw-yshuang/SeqFeedNet/main/utils/data_process.py)

GMOS reports 0.26 seconds/frame for GMOS-S and 0.38 seconds/frame for full GMOS on
an RTX A6000, excluding data loading. These figures put it in a different compute
class from the present CPU detector. Its motion-segmentation benchmarks are not
interchangeable with CDnet background-subtraction scores. The paper's release
page was inaccessible during this research, so usable code/weights were not
verified.
[GMOS runtime table and Appendix B](https://arxiv.org/html/2605.30352v1)

SAM 3.1 provides modern video segmentation/tracking, but tracking an object's mask
is not equivalent to deciding whether that object is moving independently of the
camera. It would need an actual motion decision stage. It should not be bolted on
and labeled a complete motion algorithm.
[Meta SAM 3.1 announcement](https://ai.meta.com/blog/segment-anything-model-3/)

## CPU architecture and implementation policy

Integrate reference CPU code through a narrow interface instead of implementing a
loosely similar algorithm and assigning it the published name. LITIV supplies
SuBSENSE/PAWCS; BGSLibrary also exposes them with OpenCV compatibility. Pin the
selected revision and dependencies, preserve attribution, and compare its raw
mask against the upstream implementation before modifying performance behavior.
[LITIV algorithms](https://github.com/plstcharles/litiv),
[BGSLibrary compatibility](https://github.com/andrewssobral/bgslibrary)

The inspected SuBSENSE and PAWCS headers carry Apache-2.0 notices. SeqFeedNet's
repository carries an MIT license. Model weights and transitive components still
need to be identified separately when packaging. RT-SBS is not an interchangeable
permissive implementation: its authors identify custom non-permissive terms.
[SuBSENSE source notice](https://raw.githubusercontent.com/plstcharles/litiv/master/modules/video/include/litiv/video/BackgroundSubtractorSuBSENSE.hpp),
[PAWCS source notice](https://raw.githubusercontent.com/plstcharles/litiv/master/modules/video/include/litiv/video/BackgroundSubtractorPAWCS.hpp),
[SeqFeedNet license](https://raw.githubusercontent.com/tw-yshuang/SeqFeedNet/main/LICENSE),
[RT-SBS upstream repository](https://github.com/cioppaanthony/rt-sbs)

Use runtime CPU dispatch for accelerated paths and retain baseline x86-64/AArch64
support. Useful kernels include sample-distance comparisons, local binary texture
descriptors and Hamming distances, mask processing, and image conversion. Do not
assume the current brightness-only SIMD loop is the right optimization for a new
algorithm. Measure per-camera memory as well as CPU: sample-based models retain
considerably more state than one background image.

## Integration shape

Keep media decoding and event transport separate from the foreground model:

```text
decoded frame + timestamp
    -> reference foreground model
    -> foreground mask + initialization/health state
    -> include/exclude masks and component analysis
    -> temporal event qualification
    -> existing motion events and overlays
```

The following are requirements for integration, not features currently implemented:

- Keep all-motion detection class-agnostic. Optional person/vehicle filters must
  not silently replace motion detection with a closed list of object classes.
- Preserve RGB/BGR where the selected backend benefits from color. Compare color
  and grayscale accuracy before choosing grayscale solely for throughput.
- Test 320x180 and higher analysis resolutions. The current effective 160x90
  image must not become a fixed constraint that erases small or distant motion.
- Use actual timestamps for temporal rules and explicitly account for sampling
  rate in model adaptation. Benchmark the exact 5 fps input path, not only the
  native frame-rate benchmark videos.
- Treat startup, reconnect, format changes, scene changes, and PTZ as explicit
  model lifecycle events. Test recovery rather than suppressing every large change.
- Retain zone precedence, event start/update/end semantics, and drawing expiry.
  Recalibrate sensitivity and area settings against the new mask: a foreground
  pixel fraction and the old fraction of changed blocks are different quantities.
- Apply component filtering and morphology conservatively; they can remove real
  small objects. Evaluate raw masks and final events independently.
- Run each accepted backend at its supported cadence. A cheap preliminary motion
  gate can prevent a stronger backend from seeing exactly the difficult events it
  was added to detect; any gating must pass recall tests.
- Report the backend in status. An optional GPU backend must not silently fall
  back to the legacy detector when unavailable.

## Qualification before choosing the default

Use both public ground truth and recorded surveillance video. CDnet covers
dynamic backgrounds, shadows, camera jitter, intermittent motion, night scenes,
weather, low frame rates, PTZ, thermal video, and turbulence. Apply its evaluation
masks correctly and keep test videos separate from training and parameter tuning.
[CDnet dataset](https://changedetection.net/)

For each candidate, record:

1. Foreground precision, recall, F1, and mask overlap, per category.
2. Event recall and false alarms per camera-hour at matched operating points.
3. Time to first detection, brief-event recall, and event fragmentation.
4. Behavior for large nearby objects, distant/small motion, long stationary
   foreground, object removal, lights/IR mode changes, compression artifacts,
   stream gaps, and model warmup.
5. End-to-end latency, CPU time, resident memory, and sustained camera capacity,
   including decoder and conversion overhead.
6. Results on baseline x86-64, AVX2, and AArch64 hardware, with and without optional
   GPU processing where available.

Select the strongest feasible backend at a matched false-alarm rate while meeting
the supported machine's processing budget. Report confidence intervals where
the labeled sample size permits them. Do not assert superiority from one overall
pixel F1 or a synthetic moving rectangle.

Existing public results justify this shortlist, but they do not prove that any
candidate is universally best for NeuroWatch. The first CPU comparison now covers
three labeled clips, three color seeds, and a grayscale check. It supports the
classical shortlist but does not establish production false-alarm rates. No neural
inference was run. The next work is reference-backed CPU integration and separate
camera qualification, with optional learned analysis held to the same quality
criteria.
