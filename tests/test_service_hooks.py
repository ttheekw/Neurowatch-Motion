"""Exercise package hooks without touching the machine's real user sessions."""
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import unittest


class SessionHooksTests(unittest.TestCase):
    def run_hook(self, action, *, session=True, root=True, fail_start=False):
        with tempfile.TemporaryDirectory(prefix="nw-motion-hooks-") as folder:
            work = Path(folder)
            runtime = work / "run/user/1000"
            runtime.mkdir(parents=True)
            commands = work / "bin"
            commands.mkdir()
            log = work / "calls"
            scripts = {
                "id": f"echo {0 if root else 1000}\n",
                "stat": "echo 1000\n",
                "getent": "echo 'test-user:x:1000:1000::/home/test-user:/bin/sh'\n",
                "runuser": 'printf "%s\\n" "$*" >> "$NW_HOOK_LOG"\n'
                + ('exit 1\n' if fail_start else ''),
                "systemctl": "exit 0\n",
            }
            for name, body in scripts.items():
                path = commands / name
                path.write_text("#!/bin/sh\n" + body)
                path.chmod(0o755)
            source = Path(__file__).resolve().parents[1] / "scripts/packaging/session-service.sh"
            hook = work / "hook.sh"
            hook.write_text(source.read_text().replace("/run/user/", str(work / "run/user") + "/"))
            with socket.socket(socket.AF_UNIX) as bus:
                if session:
                    bus.bind(str(runtime / "bus"))
                env = {**os.environ, "PATH": f"{commands}:{os.environ['PATH']}", "NW_HOOK_LOG": str(log)}
                subprocess.run(["sh", "-ec", '. "$1"; nw_motion_sessions "$2"', "test", str(hook), action],
                               env=env, check=True)
            calls = log.read_text().splitlines() if log.exists() else []
            for call in calls:
                self.assertIn(f"-u test-user -- env XDG_RUNTIME_DIR={runtime}", call)
                self.assertIn(f"DBUS_SESSION_BUS_ADDRESS=unix:path={runtime}/bus systemctl --user", call)
            return [call.split("systemctl --user ", 1)[1] for call in calls]

    def test_install_reloads_and_starts_the_existing_user_session(self):
        self.assertEqual(self.run_hook("install"),
                         ["daemon-reload", "try-restart nw-motion.service", "start nw-motion.service"])

    def test_removal_stops_the_service_and_reload_never_restarts_it(self):
        self.assertEqual(self.run_hook("stop"), ["daemon-reload", "stop nw-motion.service"])
        self.assertEqual(self.run_hook("reload"), ["daemon-reload"])

    def test_containers_and_unprivileged_invocations_do_not_touch_user_managers(self):
        self.assertEqual(self.run_hook("install", session=False), [])
        self.assertEqual(self.run_hook("install", root=False), [])

    def test_unavailable_user_manager_does_not_break_package_installation(self):
        self.assertEqual(self.run_hook("install", fail_start=True), ["daemon-reload"])


if __name__ == "__main__":
    unittest.main()
