# CPU motion comparison

Runs unchanged, checksum-pinned BGSLibrary SuBSENSE/PAWCS implementations, OpenCV
MOG2/KNN defaults, and the actual Rust production detector. This is an evaluation
tool; it does not change the plugin or its dependencies.

Linux prerequisites: Python 3.9+, Rust, a C++17 compiler, pkg-config, and OpenCV
development libraries (core, imgproc, imgcodecs, features2d, video). The tool was
tested with OpenCV 4.6.0. Python OpenCV, NumPy, PyTorch, and CUDA are not required.

```sh
python3 tools/motion-eval/compare.py build
```

The build downloads only the source subset listed in `upstream.json`, at the
recorded commit, and checks every SHA-256. Downloads, licenses, compiled artifacts,
and build metadata live in ignored `build/motion-eval/`. No mutable branch is used
for builds. BGSLibrary is an established integration of these algorithms, not a
new implementation of their papers. Its selected revision's license is downloaded
with the sources. Source algorithms are neither retuned nor patched by this tool.

## Real video

Download/unpack sequences from the [official CDnet dataset](https://changedetection.net/dataset2014/).
Each sequence directory must contain `input/inNNNNNN.jpg`,
`groundtruth/gtNNNNNN.png` (BMP and other OpenCV image formats also work),
`ROI.bmp`, and `temporalROI.txt`.

```sh
python3 tools/motion-eval/compare.py run \
  --sequence /path/to/pedestrians --source-fps 25 --analysis-fps 5 \
  --out build/motion-eval/runs/pedestrians-color-seed1
```

`--source-fps` is REQUIRED: filenames contain frame numbers, not reliable capture
timestamps. The example specifies a 25 fps replay assumption; it does not assert
that this clip was captured at 25 fps. Record the real rate when known. The cap
selects the first source frame in each analysis-time bucket, with no duplicated
frames. All selected frames before temporalROI are processed for causal warmup.
Scoring includes both temporalROI endpoints. `--limit` caps processed frames,
including warmup, and explicitly produces a partial comparison.

Defaults: color, 320x180, one OpenCV thread, OpenCL off, seed 1. Each algorithm
runs in a separate process, so global random state and non-thread-safe reference
code cannot interfere across models. Run more seeds and repeat timings before
capacity planning. Existing nonempty result directories are never overwritten.

The legacy detector always receives 320x180 grayscale, performs its original
160x90 smoothing, and exports its changed blocks as a coarse mask. Its actual
score and active state are preserved in CSV. New algorithms get color by default;
use `--color gray` for a controlled grayscale comparison. To test 640x360 or other
dimensions, omit `legacy` from `--algorithms`; it has a fixed production input size.

## Results

Each algorithm produces native-image-size binary PNG masks (`binNNNNNN.png`),
per-frame `frames.csv`, and `runtime.txt`. The enclosing directory contains:

- `run.json`: arguments, source/build/binary hashes, dataset-content hash, temporal
  ROI, replay rate, fixture identification, and completion status.
- `frames.tsv`: exact sampled input/GT paths, timestamps, and scoring flags.
- `summary.json`: confusion counts, precision/recall/F1/IoU, pixel false-positive
  rate, shadow false-positive rate, initialization cost, and model CPU timings.
  Partial summaries can exist after failure; require `run.json.complete == true`.

Ground truth is read only after model inference and never controls model updates.
Inference uses the full picture. The spatial ROI is used for scoring and alert
score normalization only. Labels 0 and 50 (shadow) are background, 255 is foreground,
and 85/170 (outside/unknown) are excluded. Prediction label 127 from OpenCV shadow
detection is excluded from foreground. Predictions are restored to native size
using nearest-neighbor interpolation; ground-truth labels are never resized.
No extra morphology or component filtering is applied to any model.

The legacy mask is a coarse block visualization, not pixel segmentation. Its mask
F1 demonstrates the quality of that output; it is not a fair claim that the legacy
algorithm internally predicts pixel boundaries. Compare alert behavior separately.

Undefined ratios are JSON `null`. F1 is not reported as a perfect score for a clip
containing no positives and no predicted positives. Pixel totals are accumulated
over scored samples within this sequence. These are not official, full-rate,
category-averaged CDnet leaderboard scores.

### Alert proxies

For each area threshold (default 0.1%, 0.5%, 1%, 2%, 6%), every model's predicted
foreground fraction goes through identical elapsed-time qualification: 0.5 second
continuous qualification and 10 second cooldown by default. Qualification runs
during warmup too. It does not consult ground truth. The legacy native active
column remains separate because its settings and block scoring are different.

The evaluator derives *foreground episodes* from contiguous scored samples with
at least `--min-gt-pixels` foreground pixels (default 1). It reports episode alert
recall, sampled first-alert delay, alert-state precision/recall, false alert starts
during negative samples per scored hour, and seconds active on negative samples.
An always-active alert therefore cannot hide its background-active time. Episodes
split across annotation gaps are treated separately. Unlabeled samples never count
as observed negative time. A sample represents one nominal analysis interval.

These are explicit mask-derived proxies, not independently labeled security
events. Persistent foreground need not mean physical movement, and cooldown can
overlap later episodes. Do not compare models only by episode recall or false
start count: inspect negative active time, pixel metrics, and saved masks too.
Choose an operating point on validation scenes, then evaluate held-out scenes.

### Timing and memory

`model_cpu_ms` uses Linux process CPU time, including all model threads if any.
Resize/color conversion is measured separately. Image decode/read/write, scoring,
and metrics are outside the model timing. Constructor time and first-frame
initialization are recorded separately; timing summaries exclude the first frame
but include subsequent warmup updates. Per-process peak RSS includes OpenCV,
decode/PNG buffers, and model state; it is not incremental per-camera model memory.

This tool does not measure the daemon's decoder, JSON/IPC, live frame dropping,
or end-to-end latency. Runtime CPU-feature information and compiler versions are
recorded. The build uses portable compiler flags, with no `target-cpu=native` or
global AVX2 requirement.

## Checks

To collect repeated experiments without mixing frame rates, resolutions, source
data, color modes, or different binary builds:

```sh
python3 tools/motion-eval/summarize.py \
  build/motion-eval/runs/pedestrians-color-seed{1,2,3} \
  --out build/motion-eval/report-pedestrians
```

The report contains individual `runs.csv`, per-sequence `grouped.csv` with seed
means/ranges, and full `provenance.json`. It refuses incomplete runs, duplicate
seeds, and mixed builds. It does not combine sequence scores into an official
CDnet score or select alert thresholds on the test data.

```sh
python3 -m unittest discover -s tools/motion-eval -v
c++ -std=c++17 -O2 -I/usr/include/opencv4 \
  tools/motion-eval/test_mask_metrics.cpp -lopencv_core -o /tmp/test-mask-metrics
/tmp/test-mask-metrics
python3 tools/motion-eval/compare.py fixture --out build/motion-eval/fixtures
python3 tools/motion-eval/compare.py run \
  --sequence build/motion-eval/fixtures/moving --source-fps 5 \
  --out build/motion-eval/runs/synthetic-moving
```

Fixtures cover quiet images, a moving rectangle, recurring background dynamics,
shadows, and a large occluder. They check plumbing and illustrate failure modes;
they do not establish real-camera superiority. Downloaded benchmark images are
not redistributed in this repository.
