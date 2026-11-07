# First CPU algorithm comparison

2026-11-01. This completes the first reference-backed comparison proposed in
[`motion-analysis-quality.md`](../../motion-analysis-quality.md). It provides a
working evaluator and measured results, not a production backend replacement.

## Outcome

Both SuBSENSE and PAWCS produce much better foreground masks than the current
coarse block output on these three clips. PAWCS has the highest mean mask F1 on
each clip. SuBSENSE takes roughly half the CPU time, substantially less process
memory, and has lower background pixel false-positive rates on two of the clips.
This is a real quality/cost tradeoff; F1 alone does not select the best alarm model.
The large-occluder synthetic check also exposed a serious PAWCS failure, described
below, which strengthens the case for SuBSENSE as the first integration candidate.

Proceed with a reference-backed SuBSENSE integration as the CPU balance candidate
and retain PAWCS as the higher-mask-recall candidate. Preserve color for the quality
path. Before switching the production default, validate alert settings on separate
camera recordings, especially long negative periods, night/IR changes, and small
objects. This comparison does not establish that either is globally state of the
art in 2026 or uniformly superior across all camera conditions.

## Mask quality

Mean pixel F1 over seeds 1, 2, and 3. Higher is better. New algorithms receive
320x180 BGR; the legacy detector receives its production 320x180 grayscale and
internally analyzes 160x90, producing a coarse block mask.

| Algorithm | Pedestrians: baseline | Canoe: dynamic background | People in shade: shadows |
|---|---:|---:|---:|
| Current coarse blocks | 0.504 | 0.150 | 0.617 |
| OpenCV MOG2 | 0.781 | 0.614 | 0.818 |
| OpenCV KNN | 0.835 | 0.512 | 0.824 |
| SuBSENSE | 0.928 | 0.914 | 0.895 |
| PAWCS | 0.935 | 0.936 | 0.896 |

The legacy row measures the exported block geometry, not an internal pixel
segmentation mask. Color and effective resolution also differ from the legacy
pipeline, so this table compares practical output configurations rather than
isolating the mathematical background model as the only changed variable.

The better mask results do not imply identical alarm improvements. For example,
with a 1% area threshold and 10 second cooldown, all five methods alert during
both foreground episodes in the baseline clip, but also remain active on roughly
6.4–6.6 seconds of its negative samples. The existing long cooldown dominates this
example. Alert proxies and threshold sweeps are retained in `provenance.json`;
thresholds have not been selected on these test clips.

## CPU and memory

Intel Core i7-8650U, Linux x86-64, Rust 1.94.1, GCC 13.3.0, OpenCV 4.6.0. One
OpenCV thread, OpenCL disabled, one algorithm per isolated process. Values below
are ranges across the three clips of the mean per-run median CPU time, or
per-process peak RSS, using the three color seeds.

| Algorithm | Model CPU milliseconds / analyzed frame | Peak process RSS, MiB |
|---|---:|---:|
| Current detector + block export | 0.12–0.12 | 56 |
| OpenCV MOG2 | 1.45–2.22 | 62 |
| OpenCV KNN | 1.44–3.83 | 61 |
| SuBSENSE | 24.1–31.8 | 87 |
| PAWCS | 53.3–66.6 | 202 |

These CPU costs are model-only: frame preparation, file decoding, PNG writing,
scoring, and the daemon's media/IPC are excluded. Preprocessing and first-frame
initialization are recorded separately. Peak RSS includes shared libraries,
decode/write buffers, and model state; it is not per-camera incremental memory.
Multiplying these median costs by 5 analyzed frames/second suggests approximately
12–16% of one CPU core for SuBSENSE's model alone, versus 27–33% for PAWCS. This is
a median-based illustration, not a multi-camera capacity measurement.

The reference sources use global C random-number calls and identify thread-safety
limitations. Before placing instances inside the plugin's concurrent camera
workers, verify instance isolation, RNG behavior, contention, memory scaling,
and sustained throughput. The evaluator deliberately isolates each run.

## Color must remain an accuracy decision

Grayscale was also tested with seed 1 at the same input dimensions.

| Algorithm / mode | Pedestrians F1 | Canoe F1 | People in shade F1 |
|---|---:|---:|---:|
| SuBSENSE, color (seed 1) | 0.927 | 0.914 | 0.895 |
| SuBSENSE, gray (seed 1) | 0.919 | 0.872 | 0.892 |
| PAWCS, color (seed 1) | 0.936 | 0.936 | 0.896 |
| PAWCS, gray (seed 1) | 0.897 | 0.884 | 0.837 |

Grayscale generally reduced time and memory, but did not improve F1 on these
clips; PAWCS's grayscale shadow run was also slower than its color counterpart.
Do not make grayscale a universal quality-preserving optimization.

## Synthetic regression checks

Quiet, moving-rectangle, and large-occluder fixtures were executed through all five
models. Each contains 180 frames replayed at 5 fps, with scoring from frame 61;
the object appears on frames 81–130. Inputs are synthetic grayscale textures
expanded to BGR by the common color path. These are stress/plumbing checks, not
evidence of real-camera benchmark superiority.

| Algorithm | Moving rectangle F1 | Large occluder F1 |
|---|---:|---:|
| Current coarse blocks | 0.817 | 0.000 |
| OpenCV MOG2 | 0.987 | 0.551 |
| OpenCV KNN | 1.000 | 0.895 |
| SuBSENSE | 0.968 | 0.906 |
| PAWCS | 0.976 | 0.228 |

All models produced zero false-positive pixels in the scored quiet fixture; F1
correctly remains undefined rather than being reported as perfect. The large
occluder covers most of the image and remains foreground for 50 frames. The
current detector resets its background and misses it. PAWCS also loses much of
this foreground in its default configuration. Its good three-clip F1 therefore
does not justify adopting it blindly. Preserve this case during integration.

Per-run measurements and provenance are in `fixture-checks/`. Fixture timing
should not be extrapolated to real scenes; the reference implementation itself
notes that three identical color channels are inefficient.

## Protocol and provenance

Data: three individual sequences from the
[official CDnet download page](https://changedetection.net/dataset2014/).
`datasets.json` records the exact archive URLs, byte sizes, and SHA-256 values.
Images are not redistributed here.

| Sequence | Source frames | Sampled frames | Scored samples |
|---|---:|---:|---:|
| pedestrians | 1,099 | 220 | 160 |
| canoe | 1,189 | 238 | 78 |
| peopleInShade | 1,199 | 240 | 190 |

Replay assumes 25 source frames/second and analyzes the first frame in each 5 fps
time bucket. The source capture rate was not independently verified; this is an
explicit replay assumption. All selected pre-temporalROI frames are processed to
initialize models causally. TemporalROI is inclusive. Spatial ROI is applied at
evaluation. Unknown/outside pixels are ignored; shadow labels count as background.
Foreground predictions are restored to native image size by nearest-neighbor
interpolation without resizing ground-truth labels.

SuBSENSE/PAWCS come unchanged from BGSLibrary commit
`5acc575246716005fc911384e9ca0ab25ea52de5`. OpenCV methods use defaults. RNGs
are seeded explicitly. The legacy bridge compiles the actual `src/detector.rs`;
its SHA-256 remains
`b2b704198e2ab41771bb5e473b855beecbc6218c73f9e3e135a582f6da44971c`.

There are 12 sequence/mode/seed runs and 60 algorithm results. `runs.csv` contains
each result, `grouped.csv` contains seed means/ranges without pooling incompatible
settings, and `provenance.json` includes source/binary hashes, build commands,
dataset hashes, runtime details, mask counts, and alert-proxy operating points.
Full per-frame CSV and native-size prediction masks are retained locally under
`build/motion-eval/runs/`.

Published provenance replaces the original machine's checkout prefix with relative
paths and its temporary dataset directory with `<dataset-root>/`. Measurements
and source/binary hashes are preserved.

This is a small sampled comparison, not an official CDnet leaderboard result or
an independent unseen-camera validation. These sequences are part of the methods'
historical benchmark ecosystem. They contain too little negative camera time to
establish a dependable false-alarm rate per hour. Neural methods, camera-motion
compensation, full category coverage, ARM hardware, and multi-camera load have not
been evaluated here.

## Reproduce

See [`tools/motion-eval/README.md`](../../../tools/motion-eval/README.md) for build,
download, scoring definitions, tests, and fixture instructions. For each unpacked
sequence, run seeds 1/2/3 in color and seed 1 in gray:

```sh
python3 tools/motion-eval/compare.py build
python3 tools/motion-eval/compare.py run \
  --sequence /path/to/pedestrians --source-fps 25 --analysis-fps 5 \
  --color color --seed 1 --out build/motion-eval/runs/pedestrians-color-seed1
```

Use a fresh output directory for every experiment. The build downloads only the
checksum-pinned algorithm subset into the ignored build directory. It does not
alter production sources or the plugin's Cargo dependencies.
