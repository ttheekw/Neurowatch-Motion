"""Static checks of the release workflow; nothing is run."""
from pathlib import Path
import re
import unittest

WORKFLOWS = Path(__file__).resolve().parents[1] / ".github/workflows"
RELEASE = (WORKFLOWS / "release.yml").read_text()


def job(name):
    """The text of one job: from its key to the next job's (or the end)."""
    start = RELEASE.index(f"\n  {name}:\n")
    following = re.search(r"\n  [\w-]+:\n", RELEASE[start + 1:])
    return RELEASE[start:start + 1 + following.start()] if following else RELEASE[start:]


class ReleaseWorkflowTests(unittest.TestCase):
    def test_a_manual_run_builds_every_package_and_publishes_nothing(self):
        triggers = RELEASE[RELEASE.index("\non:\n"):RELEASE.index("\npermissions:\n")]
        self.assertIn("workflow_dispatch:", triggers)
        self.assertIn("tags:\n      - 'v*'", triggers)
        # Only a pushed version tag publishes; a manual run started from a tag does not.
        self.assertIn("if: github.event_name == 'push' && startsWith(github.ref, 'refs/tags/v')",
                      job("publish"))
        for name in ("package-linux", "package-windows", "install-linux"):
            self.assertNotIn("\n    if:", job(name), name)

    def test_only_the_publishing_job_may_write_or_sign(self):
        self.assertIn("\npermissions:\n  contents: read\n", RELEASE)
        for name in ("version", "package-linux", "package-windows", "install-linux"):
            self.assertNotIn("permissions:", job(name), name)
        self.assertIn("permissions:\n      contents: write\n", job("publish"))
        self.assertIn("id-token: write", job("publish"))

    def test_the_checksums_are_signed_before_they_are_uploaded(self):
        publish = job("publish")
        sums = publish.index("xargs sha256sum > SHA256SUMS")
        sign = publish.index("cosign sign-blob --yes --bundle SHA256SUMS.sigstore.json SHA256SUMS")
        upload = publish.index("gh release create")
        self.assertLess(sums, sign)
        self.assertLess(sign, upload)

    def test_the_clean_distributions_are_pulled_by_digest(self):
        install = job("install-linux")
        images = re.findall(r"^\s+- \{image: '([^']+)'(.*)\}$", install, re.MULTILINE)
        self.assertEqual(len(images), 10)
        digests = {}
        for image, rest in images:
            found = re.search(r"digest: '(sha256:[0-9a-f]{64})'", rest)
            self.assertIsNotNone(found, image)
            # One tag, one digest: the multi-architecture index both runners pull.
            self.assertEqual(digests.setdefault(image, found.group(1)), found.group(1), image)
        self.assertIn("IMAGE: ${{ matrix.image }}@${{ matrix.digest }}", install)

    def test_every_action_is_pinned_to_a_commit(self):
        # A tag or branch can be moved to other code, which would then run in
        # the jobs that build and publish the packages.
        for workflow in sorted(WORKFLOWS.glob("*.yml")):
            for line in workflow.read_text().splitlines():
                if "uses:" in line and "uses: ./" not in line:
                    self.assertRegex(line, r"uses: [\w.-]+/[\w./-]+@[0-9a-f]{40} # \S+$",
                                     f"{workflow.name}: {line.strip()}")


if __name__ == "__main__":
    unittest.main()
