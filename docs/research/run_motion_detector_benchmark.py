#!/usr/bin/env python3
"""Research only: benchmark real detector code and two temporary variants on Linux.

Uses rustc directly, with equivalent settings structs, so GStreamer and sibling
Cargo dependencies are unnecessary. Does not modify production sources.
"""
import argparse
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--iterations", type=int, default=1000)
    parser.add_argument("--native", action="store_true")
    parser.add_argument("--cpu", type=int)
    args = parser.parse_args()
    if platform.system() != "Linux":
        parser.error("the harness uses Linux CLOCK_THREAD_CPUTIME_ID")
    if args.iterations < 1:
        parser.error("--iterations must be positive")

    here = Path(__file__).resolve().parent
    detector = here.parents[1] / "src" / "detector.rs"
    original = detector.read_text()
    old_zone = """        let zones = self
            .zones
            .iter()
            .filter(|(_, zone)| {"""
    new_zone = """        let zones = self
            .zones
            .iter()
            .filter(|_| !blocks.is_empty())
            .filter(|(_, zone)| {"""
    old_div = """            self.background[i] =
                (current + (target - current) / if changed[i] { 256 } else { 32 }) as u16;"""
    new_div = """            let delta = target - current;
            let step = if changed[i] { delta / 256 } else { delta / 32 };
            self.background[i] = (current + step) as u16;"""
    if original.count(old_zone) != 1 or original.count(old_div) != 1:
        parser.error("detector source changed; review prototype substitutions")

    with tempfile.TemporaryDirectory(prefix="nw-motion-benchmark-") as folder:
        work = Path(folder)
        shortcut = work / "shortcut.rs"
        constant_div = work / "constant_div.rs"
        shortcut.write_text(original.replace(old_zone, new_zone))
        constant_div.write_text(shortcut.read_text().replace(old_div, new_div))
        harness = (here / "motion-detector-benchmark.rs.in").read_text()
        for token, path in [("__ORIGINAL__", detector), ("__SHORTCUT__", shortcut),
                            ("__CONSTANT_DIV__", constant_div)]:
            harness = harness.replace(token, json.dumps(str(path)))
        source = work / "benchmark.rs"
        source.write_text(harness)
        binary = work / "benchmark"
        cmd = ["rustc", "--edition=2021", "-C", "opt-level=3", "-C",
               "codegen-units=1"]
        if args.native:
            cmd += ["-C", "target-cpu=native"]
        subprocess.run(cmd + [str(source), "-o", str(binary)], check=True)
        affinity = os.sched_getaffinity(0)
        cpu = args.cpu if args.cpu is not None else min(affinity)
        if cpu not in affinity:
            parser.error(f"CPU {cpu} is outside allowed affinity {sorted(affinity)}")
        os.sched_setaffinity(0, {cpu})
        print(f"Linux thread CPU time; CPU={cpu}; native={args.native}; "
              f"iterations={args.iterations}; five interleaved trials", file=sys.stderr)
        subprocess.run([str(binary), str(args.iterations)], check=True)


if __name__ == "__main__":
    main()
