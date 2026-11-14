#!/usr/bin/env python3
"""Exercise demo teardown ownership using isolated fixture processes and files."""
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import tempfile
import unittest

SOURCE = Path(os.environ.get("HYPERLANE_DEMO_SOURCE", Path(__file__).resolve().parents[1] / "demo"))


class LifecycleTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="hyperlane-lifecycle-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.demo = self.root / "demo"
        self.demo.mkdir()
        for name in ["stop-env.sh", "rusk-processes.py"]:
            if (SOURCE / name).exists():
                shutil.copy2(SOURCE / name, self.demo / name)
        self.state = self.root / "test genesis.state"
        self.pids = self.root / "services.pids"
        self.explorer = self.root / "explorer"
        values = {"PID_FILE": self.pids, "RUSK_STATE": self.state, "RUSK_HTTP_PORT": 65530,
                  "DUSK_EXPLORER_PORT": 65529, "EXPLORER_DIR": self.explorer}
        (self.demo / ".env.bridge").write_text("".join(f"{key}={shlex.quote(str(value))}\n" for key, value in values.items()))
        self.trace = self.root / "port-cleanup.txt"
        self.bin = self.root / "bin"
        self.bin.mkdir()
        stub = self.bin / "fuser"
        stub.write_text('#!/bin/sh\nprintf "%s\\n" "$*" >> "$FIXTURE_TRACE"\n')
        stub.chmod(0o700)
        self.env = {**os.environ, "PATH": str(self.bin) + os.pathsep + os.environ["PATH"],
                    "FIXTURE_TRACE": str(self.trace), "PYTHONDONTWRITEBYTECODE": "1"}

    def stop(self, entries="rusk:external\ndusk-explorer:external\n"):
        self.pids.write_text(entries)
        return subprocess.run(["bash", str(self.demo / "stop-env.sh"), "--force"],
                              env=self.env, capture_output=True, text=True, timeout=15)

    def test_external_explorer_is_not_selected_by_port(self):
        result = self.stop("rusk:external\n")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse(self.trace.exists(), "unowned service must not be selected by port")

    def test_missing_explorer_backup_is_not_an_error(self):
        self.explorer.mkdir()
        result = self.stop()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse(self.pids.exists())

    def test_unowned_explorer_configuration_is_preserved(self):
        self.explorer.mkdir()
        (self.explorer / ".env.backup.1").write_text("old unrelated config\n")
        (self.explorer / ".env").write_text("current external config\n")
        result = self.stop()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual((self.explorer / ".env").read_text(), "current external config\n")
        self.assertTrue((self.explorer / ".env.backup.1").exists())

    def test_existing_backup_is_restored(self):
        self.explorer.mkdir()
        backup = self.explorer / ".env.backup.1"
        backup.write_text("previous config\n")
        unrelated = self.explorer / ".env.backup.2"
        unrelated.write_text("unrelated config\n")
        os.utime(backup, (1, 1))
        os.utime(unrelated, (2, 2))
        (self.explorer / ".env").write_text("demo config\n")
        result = self.stop(f"dusk-explorer-env:backup:{backup}\n")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual((self.explorer / ".env").read_text(), "previous config\n")
        self.assertTrue(unrelated.exists(), "only the recorded backup may be restored")

    def test_configuration_created_by_this_run_is_removed(self):
        self.explorer.mkdir()
        config = self.explorer / ".env"
        config.write_text("demo config\n")
        result = self.stop(f"dusk-explorer-env:created:{config}\n")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse(config.exists())

    def test_owned_process_group_is_stopped_without_touching_other_processes(self):
        owned = subprocess.Popen(["sleep", "60"], start_new_session=True)
        other = subprocess.Popen(["sleep", "60"])
        try:
            result = self.stop(f"dusk-explorer:group:{owned.pid}\n")
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            owned.wait(timeout=2)
            self.assertIsNone(other.poll())
            self.assertFalse(self.trace.exists())
        finally:
            for child in [owned, other]:
                if child.poll() is None:
                    child.terminate()
                child.wait(timeout=2)

    def test_stale_wrapper_pid_stops_only_the_matching_rusk_state(self):
        source = self.root / "fixture.c"
        source.write_text('#include <unistd.h>\nint main(void) { for (;;) pause(); }\n')
        executable = self.bin / "rusk"
        subprocess.run(["cc", str(source), "-o", str(executable)], check=True, capture_output=True)
        children = []
        try:
            for args in [["-s", str(self.state)], ["--state", str(self.state) + ".other"],
                         ["--log-filter", str(self.state)]]:
                children.append(subprocess.Popen([str(executable), *args]))
            result = self.stop("rusk:99999999\n")
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            children[0].wait(timeout=2)
            self.assertIsNone(children[1].poll(), "different state must remain running")
            self.assertIsNone(children[2].poll(), "matching text under another flag must remain running")
        finally:
            for child in children:
                if child.poll() is None:
                    child.terminate()
                child.wait(timeout=2)


if __name__ == "__main__":
    unittest.main()
