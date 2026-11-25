import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import momso_anywhere_backup as backup


class ReservationTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        for name in (".momso-reservation-state.lock", ".momso-reservation-operation.lock"):
            fd = os.open(self.root / name, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
            os.close(fd)

    def tearDown(self): self.tmp.cleanup()

    def test_another_process_cannot_reserve_until_first_exits(self):
        code = ("import sys; from pathlib import Path; import momso_anywhere_backup as b; "
                "c=b.reserve_large_write(Path(sys.argv[1]),1024); c.__enter__(); "
                "print('ready',flush=True); sys.stdin.readline(); c.__exit__(None,None,None)")
        child = subprocess.Popen([sys.executable, "-c", code, str(self.root)],
                                 stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True,
                                 env={**os.environ, "PYTHONPATH": str(Path(__file__).parent)})
        try:
            self.assertEqual(child.stdout.readline().strip(), "ready")
            with self.assertRaisesRegex(ValueError, "reservation_busy"):
                with backup.reserve_large_write(self.root, 1): pass
            self.assertEqual((self.root / ".momso-reservation-operation.lock").read_text(), "1024\n")
            child.stdin.write("finish\n"); child.stdin.flush()
            self.assertEqual(child.wait(timeout=5), 0)
            with backup.reserve_large_write(self.root, 1): pass
        finally:
            if child.poll() is None: child.kill(); child.wait()
            child.stdin.close(); child.stdout.close()

    def test_low_disk_rejected_and_symlink_not_followed(self):
        with patch.object(backup.shutil, "disk_usage", return_value=type("Usage", (), {"free": 100})()):
            with self.assertRaisesRegex(ValueError, "disk_low"):
                with backup.reserve_large_write(self.root, 1): pass
        operation = self.root / ".momso-reservation-operation.lock"
        operation.unlink()
        operation.symlink_to(self.root / "outside")
        with self.assertRaisesRegex(ValueError, "reservation_unavailable"):
            with backup.reserve_large_write(self.root, 1): pass


if __name__ == "__main__": unittest.main()
