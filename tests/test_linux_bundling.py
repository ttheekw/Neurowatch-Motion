"""Build tiny ELF fixtures and exercise the actual portable package script."""
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]


@unittest.skipUnless(sys.platform.startswith("linux") and shutil.which("cc"), "requires Linux and a C compiler")
class LinuxBundlingTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="nw-motion-bundle-")
        self.addCleanup(self.temporary.cleanup)
        self.work = Path(self.temporary.name)
        self.root = self.work / "project"
        self.sdk = self.work / "sdk"
        self.system = self.work / "system"
        self.target = self.work / "target"
        (self.root / "scripts/packaging").mkdir(parents=True)
        for name in ("LICENSE", "NOTICE", "README.md", "scripts/package_linux.sh",
                     "scripts/packaging/nw-motion.service"):
            shutil.copyfile(ROOT / name, self.root / name)
        (self.root / "Cargo.toml").write_text('[package]\nname = "nw-motion"\nversion = "0.2.0"\n')
        for folder in (self.sdk / "lib/gstreamer-1.0", self.sdk / "share/licenses/neurowatch-media",
                       self.system / "lib", self.system / "share/doc/liborc-0.4-0t64", self.target / "release"):
            folder.mkdir(parents=True)
        (self.sdk / "VERSIONS.txt").write_text("Fixture SDK\n")
        (self.sdk / "share/licenses/neurowatch-media/gstreamer.txt").write_text("Fixture SDK license\n")
        self.copyright = self.system / "share/doc/liborc-0.4-0t64/copyright"
        self.copyright.write_text("Fixture ORC runtime license\n")

        self.compile("int orc_fixture(void) { return 42; }", self.system / "lib/liborc-0.4.so.0",
                     "-shared", "-fPIC", "-Wl,-soname,liborc-0.4.so.0")
        self.compile("extern int orc_fixture(void); int audio_fixture(void) { return orc_fixture(); }",
                     self.sdk / "lib/libgstaudio-1.0.so.0", "-shared", "-fPIC",
                     "-Wl,-soname,libgstaudio-1.0.so.0", f"-L{self.system / 'lib'}", "-l:liborc-0.4.so.0")
        self.compile("extern int audio_fixture(void); int main(void) { return audio_fixture() != 42; }",
                     self.target / "release/nw-motion", str(self.sdk / "lib/libgstaudio-1.0.so.0"),
                     f"-Wl,-rpath-link,{self.system / 'lib'}",
                     "-Wl,-rpath,$ORIGIN/../gstreamer/lib", "-Wl,--disable-new-dtags")
        plugin = self.work / "plugin.so"
        self.compile("int fixture(void) { return 0; }", plugin, "-shared", "-fPIC")
        for name in ("coreelements", "app", "playback", "typefindfunctions", "videoconvertscale",
                     "videoparsersbad", "libav"):
            shutil.copyfile(plugin, self.sdk / f"lib/gstreamer-1.0/libgst{name}.so")

        commands = self.work / "bin"
        commands.mkdir()
        cargo = commands / "cargo"
        cargo.write_text("#!/bin/sh\nexit 0\n")  # The fixture executable is already compiled above.
        cargo.chmod(0o755)
        pkg_config = commands / "pkg-config"
        pkg_config.write_text("#!/bin/sh\ncase \"$*\" in\n"
                              f"'--variable=libdir orc-0.4') echo {shlex.quote(str(self.system / 'lib'))} ;;\n"
                              f"'--variable=prefix orc-0.4') echo {shlex.quote(str(self.system))} ;;\n"
                              "'--modversion glib-2.0') echo 2.70 ;;\n*) exit 1 ;;\nesac\n")
        pkg_config.chmod(0o755)
        self.env = {**os.environ, "PATH": f"{commands}:{os.environ['PATH']}",
                    "CARGO_TARGET_DIR": str(self.target)}
        for name in ("NW_MOTION_LENIENT_DEPS", "LD_LIBRARY_PATH", "PKG_CONFIG_PATH"):
            self.env.pop(name, None)

    def compile(self, source, output, *args):
        subprocess.run(["cc", "-x", "c", "-", "-x", "none", *args, "-o", str(output)],
                       input=source, text=True, capture_output=True, check=True)

    def package(self):
        return subprocess.run(["bash", str(self.root / "scripts/package_linux.sh"),
                               "--version", "v0.2.0", "--arch", os.uname().machine,
                               "--format", "portable", "--gstreamer-sdk", str(self.sdk)],
                              env=self.env, text=True, capture_output=True)

    def assert_bundle_runs(self, result):
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        archive_path = next((self.root / "dist/release").glob("*.tar.gz"))
        destination = self.work / "extracted"
        with tarfile.open(archive_path) as archive:
            archive.extractall(destination, filter="data")
        bundle = destination / "nw-motion"
        # The executable really calls the SDK library, which calls the ORC
        # fixture. Successful execution proves the entire dependency chain loads.
        subprocess.run([str(bundle / "bin/nw-motion"), "--check"], env=self.env, check=True)
        self.assertTrue((bundle / "gstreamer/lib/liborc-0.4.so.0").is_file())
        return bundle

    def test_sdk_using_system_orc_bundles_runtime_and_license(self):
        bundle = self.assert_bundle_runs(self.package())
        self.assertEqual((bundle / "gstreamer/licenses/orc.txt").read_text(), self.copyright.read_text())

    def test_sdk_bundled_orc_stays_independent_of_system_orc(self):
        shutil.copyfile(self.system / "lib/liborc-0.4.so.0", self.sdk / "lib/liborc-0.4.so.0")
        (self.sdk / "share/licenses/neurowatch-media/orc.txt").write_text("SDK ORC license\n")
        shutil.rmtree(self.system)
        bundle = self.assert_bundle_runs(self.package())
        self.assertEqual((bundle / "gstreamer/licenses/orc.txt").read_text(), "SDK ORC license\n")

    def test_missing_system_orc_license_fails_packaging(self):
        self.copyright.unlink()
        result = self.package()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("ORC runtime copyright missing", result.stderr)


if __name__ == "__main__":
    unittest.main()
