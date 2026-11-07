"""Metric/protocol tests: run with python3 -m unittest discover -s tools/motion-eval."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("motion_compare", Path(__file__).with_name("compare.py"))
compare = importlib.util.module_from_spec(spec)
spec.loader.exec_module(compare)
report_spec = importlib.util.spec_from_file_location("motion_report", Path(__file__).with_name("summarize.py"))
report = importlib.util.module_from_spec(report_spec)
report_spec.loader.exec_module(report)


class ProtocolTests(unittest.TestCase):
    def test_sampling_keeps_warmup_and_inclusive_temporal_roi(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            (root / "input").mkdir()
            (root / "groundtruth").mkdir()
            for n in range(1, 12):
                (root / "input" / f"in{n:06}.jpg").touch()
                (root / "groundtruth" / f"gt{n:06}.png").touch()
            (root / "temporalROI.txt").write_text("6 11\n")
            rows, _, count = compare.frame_manifest(root, 25, 5)
            self.assertEqual(count, 11)
            self.assertEqual([r[:3] for r in rows], [(1, 0, 0), (6, 200, 1), (11, 400, 1)])
            with self.assertRaisesRegex(ValueError, "warmup"):
                compare.frame_manifest(root, 25, 5, limit=1)
            (root / "groundtruth" / "gt000006.png").unlink()
            with self.assertRaisesRegex(ValueError, "missing evaluated"):
                compare.frame_manifest(root, 25, 5)

    def test_undefined_metrics_are_not_perfect_scores(self):
        result = compare.metrics(0, 0, 0, 100)
        self.assertIsNone(result["f1"])
        self.assertIsNone(result["precision"])
        self.assertIsNone(result["recall"])
        self.assertEqual(result["false_positive_rate"], 0)
        self.assertEqual(compare.metrics(3, 1, 2, 4)["f1"], 6 / 9)

    def test_qualification_uses_elapsed_time_and_continuity(self):
        rows = [{"time_ms": t, "score": s} for t, s in
                [(0, 0), (200, 1), (400, 1), (600, 0), (800, 1),
                 (1000, 1), (1400, 1), (1600, 0), (2400, 0)]]
        self.assertEqual(compare.qualify(rows, .1, 500, 1000),
                         [False, False, False, False, False, False, True, True, False])
        # Ground-truth values cannot influence qualification.
        for row in rows:
            row["gt_foreground"] = 10**6
        self.assertEqual(compare.qualify(rows, .1, 500, 1000),
                         [False, False, False, False, False, False, True, True, False])

    def test_always_active_alert_cannot_hide_negative_time(self):
        rows = [{"time_ms": i*200, "evaluated": 1, "valid_pixels": 10,
                 "gt_foreground": fg} for i, fg in enumerate([0, 0, 2, 2, 0])]
        result = compare.alert_metrics(rows, [True]*5, 200, 1)
        self.assertEqual(result["episode_alert_recall"], 1)
        self.assertEqual(result["false_alert_starts"], 1)
        self.assertEqual(result["background_active_seconds"], .6)
        self.assertEqual(result["precision"], .4)

    def test_unknown_intervals_are_never_negative_observations(self):
        rows = [{"time_ms": i*200, "evaluated": e, "valid_pixels": n,
                 "gt_foreground": g} for i, (e, n, g) in
                enumerate([(0, 10, 0), (1, 0, 0), (1, 10, 2), (1, 10, 0)])]
        result = compare.alert_metrics(rows, [False, False, True, False], 200, 1)
        self.assertEqual(result["scored_seconds"], .4)
        self.assertEqual(result["false_alert_starts"], 0)
        self.assertEqual(result["gt_foreground_episodes"], 1)

    def test_reporting_separates_settings_and_rejects_duplicate_seeds(self):
        row = dict(sequence="clip", algorithm="legacy", color="gray", legacy_sensitivity=5,
                   width=320, height=180, source_fps_assumed=25, analysis_fps_cap=5,
                   dataset_sha256="example", synthetic=False, scored_frames=10,
                   mask_kind="coarse changed blocks", seed=1,
                   f1=.8, precision=.8, recall=.8, false_positive_rate=.01,
                   median_model_cpu_ms=1, p95_model_cpu_ms=2, peak_process_rss_mib=50)
        repeated = {**row, "seed": 2, "f1": .6}
        grouped = report.aggregate([row, repeated])
        self.assertEqual(len(grouped), 1)
        self.assertEqual(grouped[0]["seeds"], 2)
        self.assertAlmostEqual(grouped[0]["f1_mean"], .7)
        with self.assertRaisesRegex(ValueError, "duplicate seeds"):
            report.aggregate([row, row])
        self.assertEqual(len(report.aggregate([row, {**row, "legacy_sensitivity": 9}])), 2)


if __name__ == "__main__":
    unittest.main()
