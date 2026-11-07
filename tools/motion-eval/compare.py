#!/usr/bin/env python3
"""Build and run a pinned, CPU-only comparison on CDnet-format sequences."""
import argparse
import csv
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import re
import shlex
import statistics
import subprocess
import sys
import urllib.request

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
BUILD = ROOT / "build" / "motion-eval"
ALGORITHMS = ("legacy", "mog2", "knn", "subsense", "pawcs")
EXTENSIONS = {".jpg", ".jpeg", ".png", ".bmp", ".pgm", ".ppm"}


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def checked(command):
    return subprocess.check_output(command, text=True).strip()


def build():
    if platform.system() != "Linux":
        raise ValueError("the evaluator currently uses Linux CPU clocks/RSS accounting")
    pin = json.loads((HERE / "upstream.json").read_text())
    upstream = BUILD / "upstream"
    upstream.mkdir(parents=True, exist_ok=True)
    for entry in pin["files"]:
        dest = upstream / Path(entry["path"]).name
        if not dest.exists():
            url = ("https://raw.githubusercontent.com/andrewssobral/bgslibrary/"
                   f"{pin['revision']}/{entry['path']}")
            print(f"Downloading {entry['path']}", file=sys.stderr)
            with urllib.request.urlopen(url, timeout=45) as response:
                content = response.read()
            if hashlib.sha256(content).hexdigest() != entry["sha256"]:
                raise ValueError(f"upstream checksum mismatch: {entry['path']}")
            dest.write_bytes(content)
        if digest(dest) != entry["sha256"]:
            raise ValueError(f"modified upstream file: {dest}")
    rustlib = BUILD / "liblegacy.a"
    subprocess.run(["rustc", "--edition=2021", "--crate-type=staticlib", "-C",
                    "opt-level=3", "-C", "codegen-units=1", str(HERE / "legacy.rs"),
                    "-o", str(rustlib)], check=True)
    flags = shlex.split(checked(["pkg-config", "--cflags", "opencv4"]))
    sources = ["BackgroundSubtractorLBSP.cpp", "BackgroundSubtractorLBSP_.cpp",
               "BackgroundSubtractorSuBSENSE.cpp", "BackgroundSubtractorPAWCS.cpp",
               "LBSP.cpp", "LBSP_.cpp"]
    compiler = os.environ.get("CXX", "c++")
    command = [compiler, "-std=c++17", "-O3", "-DNDEBUG", *flags, "-I", str(upstream),
               str(HERE / "runner.cpp"), *(str(upstream / s) for s in sources),
               str(rustlib), "-lopencv_video", "-lopencv_imgcodecs", "-lopencv_imgproc",
               "-lopencv_features2d", "-lopencv_core", "-ldl", "-lpthread", "-lm",
               "-o", str(BUILD / "motion-eval")]
    subprocess.run(command, check=True)
    metadata = {
        "upstream": pin, "rustc": checked(["rustc", "--version"]),
        "compiler": checked([compiler, "--version"]).splitlines()[0],
        "opencv": checked(["pkg-config", "--modversion", "opencv4"]),
        "platform": platform.platform(), "compile_command": command,
        "source_sha256": {str(p.relative_to(ROOT)): digest(p) for p in
                          [HERE / "runner.cpp", HERE / "mask_metrics.hpp", HERE / "legacy.rs", HERE / "compare.py",
                           ROOT / "src" / "detector.rs"]},
        "binary_sha256": digest(BUILD / "motion-eval"),
    }
    (BUILD / "build.json").write_text(json.dumps(metadata, indent=2) + "\n")
    print(f"Built {BUILD / 'motion-eval'}")


def indexed(directory, prefix):
    found = {}
    for p in directory.iterdir():
        match = re.fullmatch(re.escape(prefix) + r"(\d+)", p.stem)
        if p.is_file() and p.suffix.lower() in EXTENSIONS and match:
            number = int(match[1])
            if number in found:
                raise ValueError(f"ambiguous frame number {number}: {directory}")
            found[number] = p.resolve()
    return dict(sorted(found.items()))


def frame_manifest(sequence, source_fps, analysis_fps, limit=None):
    frames = indexed(sequence / "input", "in")
    truth = indexed(sequence / "groundtruth", "gt")
    if not frames:
        raise ValueError("no numbered input frames")
    first, last = min(frames), max(frames)
    if len(frames) != last - first + 1:
        raise ValueError("input sequence has gaps; use a complete sequence")
    temporal = [int(x) for x in (sequence / "temporalROI.txt").read_text().split()]
    if len(temporal) != 2 or temporal[0] > temporal[1]:
        raise ValueError("invalid temporalROI.txt")
    if temporal[0] < first or temporal[1] > last:
        raise ValueError("temporal ROI extends outside input sequence")
    rows = []
    last_bucket = -1
    for number, path in frames.items():
        time_ms = (number - first) * 1000.0 / source_fps
        bucket = math.floor(time_ms * analysis_fps / 1000 + 1e-9)
        if bucket == last_bucket:
            continue
        last_bucket = bucket
        evaluated = temporal[0] <= number <= temporal[1]
        if evaluated and number not in truth:
            raise ValueError(f"missing evaluated ground truth for frame {number}")
        rows.append((number, time_ms, int(evaluated), str(path),
                     str(truth[number]) if evaluated else "-"))
        if limit is not None and len(rows) >= limit:
            break
    if not any(row[2] for row in rows):
        raise ValueError("no evaluated frames; limit may end during warmup")
    return rows, temporal, len(frames)


def ratio(numerator, denominator):
    return numerator / denominator if denominator else None


def metrics(tp, fp, fn, tn):
    return {
        "precision": ratio(tp, tp + fp), "recall": ratio(tp, tp + fn),
        "f1": ratio(2 * tp, 2 * tp + fp + fn),
        "iou": ratio(tp, tp + fp + fn), "false_positive_rate": ratio(fp, fp + tn),
        "wrong_classification_percent": ratio(100 * (fp + fn), tp + fp + fn + tn),
    }


def qualify(rows, threshold, duration_ms, cooldown_ms):
    """Same elapsed-time qualification for all mask scores; no GT is consulted."""
    candidate = None
    last_motion = None
    active = False
    states = []
    for row in rows:
        now = float(row["time_ms"])
        if float(row["score"]) >= threshold:
            if candidate is None:
                candidate = now
            last_motion = now
            if now - candidate + 1e-7 >= duration_ms:
                active = True
        else:
            candidate = None
            if active and now - last_motion + 1e-7 >= cooldown_ms:
                active = False
        states.append(active)
    return states


def alert_metrics(rows, states, period_ms, min_gt_pixels):
    # These are alert proxies derived from per-frame foreground labels, not
    # manually annotated semantic events. Keep background-active time visible.
    tp = fp = fn = tn = false_starts = 0
    gt_episodes = []
    episode = []
    previous_time = None
    previous_state = False
    scored_ms = background_active_ms = 0.0
    for row, state in zip(rows, states):
        time = float(row["time_ms"])
        evaluated = int(row["evaluated"]) and int(row["valid_pixels"]) > 0
        if not evaluated:
            if episode:
                gt_episodes.append(episode)
                episode = []
            previous_time = None
            previous_state = state
            continue
        # Each scored sample represents at most one analysis interval. Missing
        # labels must not count as observed negative time.
        if previous_time is not None and time - previous_time > period_ms * 1.6:
            if episode:
                gt_episodes.append(episode)
                episode = []
        positive = int(row["gt_foreground"]) >= min_gt_pixels
        tp += state and positive; fp += state and not positive
        fn += not state and positive; tn += not state and not positive
        false_starts += state and not previous_state and not positive
        scored_ms += period_ms
        background_active_ms += period_ms if state and not positive else 0
        if positive:
            episode.append((time, state))
        elif episode:
            gt_episodes.append(episode)
            episode = []
        previous_time, previous_state = time, state
    if episode:
        gt_episodes.append(episode)
    delays = []
    for episode in gt_episodes:
        detected = [time for time, state in episode if state]
        if detected:
            delays.append(detected[0] - episode[0][0])
    return {
        **metrics(tp, fp, fn, tn), "gt_foreground_episodes": len(gt_episodes),
        "episode_alert_recall": ratio(len(delays), len(gt_episodes)),
        "median_first_alert_delay_ms": statistics.median(delays) if delays else None,
        "false_alert_starts": false_starts,
        "false_alert_starts_per_scored_hour": ratio(false_starts * 3_600_000, scored_ms),
        "background_active_seconds": background_active_ms / 1000,
        "scored_seconds": scored_ms / 1000,
    }


def percentile(values, fraction):
    values = sorted(values)
    return values[max(0, math.ceil(len(values) * fraction) - 1)] if values else None


def summarize(rows, args):
    scored = [r for r in rows if int(r["evaluated"])]
    totals = {key: sum(int(r[key]) for r in scored) for key in
              ["tp", "fp", "fn", "tn", "shadow_fp", "shadow_pixels"]}
    timings = [float(r["model_cpu_ms"]) for r in rows if not int(r["initialization"])]
    timing_rows = [r for r in rows if not int(r["initialization"])]
    return {
        "processed_frames": len(rows), "scored_frames": len(scored),
        "pixel_counts": totals,
        "pixel_metrics": {**metrics(*(totals[k] for k in ["tp", "fp", "fn", "tn"])),
                          "shadow_false_positive_rate": ratio(totals["shadow_fp"], totals["shadow_pixels"])},
        "timing": {
            "median_model_cpu_ms": statistics.median(timings) if timings else None,
            "p95_model_cpu_ms": percentile(timings, 0.95),
            "mean_model_cpu_ms": statistics.mean(timings) if timings else None,
            "median_model_wall_ms": statistics.median(float(r["model_wall_ms"]) for r in timing_rows)
                                    if timing_rows else None,
            "mean_preprocess_cpu_ms": statistics.mean(float(r["preprocess_cpu_ms"]) for r in rows),
            "first_frame_model_cpu_ms": float(rows[0]["model_cpu_ms"]),
        },
        "alert_proxy_operating_points": {
            str(threshold): alert_metrics(rows, qualify(rows, threshold, args.min_duration * 1000,
                                                       args.cooldown * 1000),
                                         1000 / min(args.analysis_fps, args.source_fps), args.min_gt_pixels)
            for threshold in args.area_thresholds
        },
    }


def run(args):
    if not (BUILD / "motion-eval").is_file():
        raise ValueError("build the runner first: python3 tools/motion-eval/compare.py build")
    if not all(math.isfinite(x) and x > 0 for x in [args.source_fps, args.analysis_fps]):
        raise ValueError("frame rates must be finite and positive")
    if not all(math.isfinite(t) and 0 < t <= 1 for t in args.area_thresholds):
        raise ValueError("area thresholds must be in (0,1]")
    if any(not math.isfinite(t) or t < 0 for t in [args.min_duration, args.cooldown]):
        raise ValueError("duration/cooldown must be finite and nonnegative")
    if args.min_gt_pixels < 1 or (args.limit is not None and args.limit < 1):
        raise ValueError("minimum GT pixels and frame limit must be positive")
    if not 0 <= args.seed <= 2**31-1:
        raise ValueError("seed must be in [0, 2147483647]")
    if len(set(args.algorithms)) != len(args.algorithms):
        raise ValueError("duplicate algorithms would overwrite results")
    if args.width < 32 or args.height < 32 or not 1 <= args.sensitivity <= 10:
        raise ValueError("invalid dimensions or sensitivity")
    if "legacy" in args.algorithms and (args.width, args.height) != (320, 180):
        raise ValueError("legacy requires --width 320 --height 180")
    sequence, output = args.sequence.resolve(), args.out.resolve()
    rows, temporal, source_frames = frame_manifest(sequence, args.source_fps, args.analysis_fps, args.limit)
    for row in rows:
        if any("\t" in str(x) or "\n" in str(x) or "\r" in str(x) for x in row):
            raise ValueError("frame paths cannot contain tabs or newlines")
    if output.exists() and any(output.iterdir()):
        raise ValueError("output directory must be new or empty; existing results are never overwritten")
    output.mkdir(parents=True, exist_ok=True)
    if not (sequence / "ROI.bmp").is_file():
        raise ValueError("missing ROI.bmp")
    manifest = output / "frames.tsv"
    manifest.write_text("".join("\t".join(map(str, row)) + "\n" for row in rows))
    build_meta = json.loads((BUILD / "build.json").read_text())
    if digest(BUILD / "motion-eval") != build_meta["binary_sha256"]:
        raise ValueError("runner binary differs from build metadata; rebuild")
    for path, expected in build_meta["source_sha256"].items():
        if digest(ROOT / path) != expected:
            raise ValueError(f"source changed since build: {path}; rebuild")
    metadata = {
        "complete": False, "algorithms": args.algorithms,
        "sequence": str(sequence), "source_fps_assumed": args.source_fps,
        "analysis_fps_cap": args.analysis_fps, "source_frames": source_frames,
        "sampled_frames": len(rows), "temporal_roi_inclusive": temporal,
        "sampled_frame_limit": args.limit,
        "input_color": args.color, "width": args.width, "height": args.height,
        "seed": args.seed, "legacy_sensitivity": args.sensitivity,
        "alert_proxy_min_duration_sec": args.min_duration, "alert_proxy_cooldown_sec": args.cooldown,
        "alert_proxy_min_gt_pixels": args.min_gt_pixels,
        "evaluation": "sampled foreground masks and derived alert proxies; not official full-rate CDnet scores",
        "manifest_sha256": digest(manifest), "build": build_meta,
        "fixture": json.loads((sequence / "fixture.json").read_text())
                   if (sequence / "fixture.json").is_file() else None,
    }
    # Hash bytes, not just paths: changed annotations or images must be detectable.
    content = hashlib.sha256()
    paths = {sequence / "ROI.bmp", sequence / "temporalROI.txt"}
    paths.update(Path(row[3]) for row in rows)
    paths.update(Path(row[4]) for row in rows if row[2])
    for path in sorted(paths):
        content.update(str(path.relative_to(sequence)).encode() + b"\0")
        content.update(bytes.fromhex(digest(path)))
    metadata["sampled_dataset_sha256"] = content.hexdigest()
    (output / "run.json").write_text(json.dumps(metadata, indent=2) + "\n")
    summaries = {}
    for algorithm in args.algorithms:
        subprocess.run([str(BUILD / "motion-eval"), algorithm, str(manifest),
                        str(sequence / "ROI.bmp"), str(output / algorithm),
                        str(args.width), str(args.height), args.color, str(args.seed),
                        str(args.sensitivity)], check=True)
        with (output / algorithm / "frames.csv").open() as stream:
            result_rows = list(csv.DictReader(stream))
        if len(result_rows) != len(rows):
            raise ValueError(f"incomplete result rows for {algorithm}")
        summaries[algorithm] = summarize(result_rows, args)
        summaries[algorithm]["mask_kind"] = "coarse changed blocks" if algorithm == "legacy" else "foreground segmentation"
        summaries[algorithm]["runtime"] = (output / algorithm / "runtime.txt").read_text()
        (output / "summary.json").write_text(json.dumps(summaries, indent=2, allow_nan=False) + "\n")
        f1 = summaries[algorithm]["pixel_metrics"]["f1"]
        ms = summaries[algorithm]["timing"]["median_model_cpu_ms"]
        print(f"{algorithm:9} pixel F1={f1!s:18} median CPU ms={ms}")
    print(f"Results: {output}")
    metadata["complete"] = True
    (output / "run.json").write_text(json.dumps(metadata, indent=2) + "\n")


def fixture(args):
    """Small deterministic plumbing fixtures; not a real-camera quality benchmark."""
    width, height, count = 320, 180, 180
    out = args.out.resolve()
    if out.exists() and any(out.iterdir()):
        raise ValueError("fixture directory must be new or empty")
    for case in ["quiet", "moving", "dynamic", "shadow", "large-object"]:
        sequence = out / case
        (sequence / "input").mkdir(parents=True)
        (sequence / "groundtruth").mkdir()
        base = bytes(50 + (x * 7 + y * 13) % 35 for y in range(height) for x in range(width))
        header = f"P5\n{width} {height}\n255\n".encode()
        # OpenCV reads PNM by content even when the CDnet ROI filename is .bmp.
        (sequence / "ROI.bmp").write_bytes(header + bytes([255]) * width * height)
        (sequence / "temporalROI.txt").write_text(f"61 {count}\n")
        (sequence / "fixture.json").write_text(json.dumps({"synthetic": True, "case": case,
                                                          "source_fps": 5, "frames": count}) + "\n")
        for index in range(count):
            frame, gt = bytearray(base), bytearray(width * height)
            if case == "dynamic":
                # Recurring background dynamics begin during warmup.
                value = 50 if index % 4 < 2 else 130
                for y in range(20, 80):
                    frame[y*width+10:y*width+70] = bytes([value]) * 60
            if 80 <= index < 130 and case != "quiet":
                x0, y0, w, h, label = 90 + (index - 80) * 2, 80, 45, 55, 255
                if case == "large-object":
                    x0, y0, w, h = 5, 5, 310, 170
                if case == "shadow":
                    label = 50
                for y in range(y0, y0+h):
                    start = y*width+x0
                    values = bytes(v // 2 for v in base[start:start+w]) if label == 50 else bytes([190]) * w
                    frame[start:start+w] = values
                    gt[start:start+w] = bytes([label]) * w
            (sequence / "input" / f"in{index+1:06}.pgm").write_bytes(header + frame)
            (sequence / "groundtruth" / f"gt{index+1:06}.pgm").write_bytes(header + gt)
    print(f"Synthetic fixtures: {out}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("build", help="download checksum-pinned sources and compile")
    generate = sub.add_parser("fixture", help="generate deterministic plumbing fixtures")
    generate.add_argument("--out", type=Path, required=True)
    compare = sub.add_parser("run", help="compare one complete CDnet-format sequence")
    compare.add_argument("--sequence", type=Path, required=True)
    compare.add_argument("--out", type=Path, required=True)
    compare.add_argument("--source-fps", type=float, required=True,
                         help="explicit source rate; recorded as an assumption, not inferred from filenames")
    compare.add_argument("--analysis-fps", type=float, default=5)
    compare.add_argument("--algorithms", nargs="+", choices=ALGORITHMS, default=list(ALGORITHMS))
    compare.add_argument("--width", type=int, default=320)
    compare.add_argument("--height", type=int, default=180)
    compare.add_argument("--color", choices=["color", "gray"], default="color")
    compare.add_argument("--seed", type=int, default=1)
    compare.add_argument("--sensitivity", type=int, default=5)
    compare.add_argument("--limit", type=int, help="cap sampled frames, including warmup; partial benchmark only")
    compare.add_argument("--area-thresholds", type=float, nargs="+", default=[.001, .005, .01, .02, .06])
    compare.add_argument("--min-duration", type=float, default=.5)
    compare.add_argument("--cooldown", type=float, default=10)
    compare.add_argument("--min-gt-pixels", type=int, default=1)
    args = parser.parse_args()
    try:
        if args.command == "build":
            build()
        elif args.command == "fixture":
            fixture(args)
        else:
            run(args)
    except (ValueError, OSError, subprocess.CalledProcessError) as exc:
        parser.exit(1, f"motion-eval: {exc}\n")


if __name__ == "__main__":
    main()
