#!/usr/bin/env python3
"""Collect completed comparison runs without pooling incompatible experiments."""
import argparse
import csv
import json
from pathlib import Path
import statistics


def collect(paths):
    records, details, builds = [], [], set()
    for path in paths:
        meta = json.loads((path / "run.json").read_text())
        if not meta["complete"]:
            raise ValueError(f"incomplete run: {path}")
        summary = json.loads((path / "summary.json").read_text())
        if set(summary) != set(meta["algorithms"]):
            raise ValueError(f"missing algorithm results: {path}")
        builds.add(meta["build"]["binary_sha256"])
        details.append({"run": str(path), "metadata": meta, "summary": summary})
        for algorithm, result in summary.items():
            rss = next(line.split("=", 1)[1] for line in result["runtime"].splitlines()
                       if line.startswith("peak_process_rss_kib="))
            records.append({
                "sequence": Path(meta["sequence"]).name, "algorithm": algorithm,
                "color": meta["input_color"], "seed": meta["seed"],
                "legacy_sensitivity": meta["legacy_sensitivity"],
                "width": meta["width"], "height": meta["height"],
                "source_fps_assumed": meta["source_fps_assumed"],
                "analysis_fps_cap": meta["analysis_fps_cap"],
                "dataset_sha256": meta["sampled_dataset_sha256"],
                "synthetic": meta["fixture"] is not None,
                "scored_frames": result["scored_frames"],
                "mask_kind": result["mask_kind"],
                **result["pixel_metrics"], **result["timing"],
                "peak_process_rss_mib": int(rss) / 1024,
            })
    if len(builds) != 1:
        raise ValueError("runs use different builds; report them separately")
    return records, details


def aggregate(records):
    groups = {}
    keys = ["sequence", "algorithm", "color", "legacy_sensitivity", "width", "height", "source_fps_assumed",
            "analysis_fps_cap", "dataset_sha256", "synthetic", "scored_frames", "mask_kind"]
    for record in records:
        groups.setdefault(tuple(record[k] for k in keys), []).append(record)
    result = []
    for key, group in sorted(groups.items()):
        if len({r["seed"] for r in group}) != len(group):
            raise ValueError("duplicate seeds for the same experiment")
        row = {**dict(zip(keys, key)), "seeds": len(group)}
        for metric in ["f1", "precision", "recall", "false_positive_rate",
                       "median_model_cpu_ms", "p95_model_cpu_ms", "peak_process_rss_mib"]:
            values = [r[metric] for r in group if r[metric] is not None]
            row[metric + "_mean"] = statistics.mean(values) if values else None
            row[metric + "_min"] = min(values) if values else None
            row[metric + "_max"] = max(values) if values else None
        result.append(row)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("runs", nargs="+", type=Path)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    try:
        records, details = collect(args.runs)
        grouped = aggregate(records)
        if args.out.exists() and any(args.out.iterdir()):
            raise ValueError("report directory must be new or empty")
        args.out.mkdir(parents=True, exist_ok=True)
        for name, rows in [("runs.csv", records), ("grouped.csv", grouped)]:
            with (args.out / name).open("w", newline="") as stream:
                writer = csv.DictWriter(stream, fieldnames=list(rows[0]))
                writer.writeheader()
                writer.writerows(rows)
        (args.out / "provenance.json").write_text(json.dumps(details, indent=2, allow_nan=False) + "\n")
        print(f"Collected {len(details)} runs, {len(records)} model results: {args.out}")
    except (ValueError, OSError, KeyError) as exc:
        parser.exit(1, f"motion-eval report: {exc}\n")


if __name__ == "__main__":
    main()
