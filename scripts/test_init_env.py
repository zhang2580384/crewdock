import base64
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("init-env.py")


def parse(path: Path) -> dict[str, str]:
    return dict(line.split("=", 1) for line in path.read_text(encoding="utf-8").splitlines() if line and not line.startswith("#"))


class InitialConfigurationTests(unittest.TestCase):
    def test_fresh_install_has_unique_secrets_and_never_overwrites_them(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            first, second = (Path(directory) / name for name in ("first.env", "second.env"))
            for output in (first, second):
                result = subprocess.run([sys.executable, str(SCRIPT), "--output", str(output), "--email", "owner@example.com"], capture_output=True, text=True, check=True)
                self.assertNotIn(parse(output)["CREWDOCK_BOOTSTRAP_PASSWORD"], result.stdout + result.stderr)
            a, b = parse(first), parse(second)
            self.assertEqual(32, len(base64.b64decode(a["CREWDOCK_MASTER_KEY"], validate=True)))
            self.assertNotEqual(bytes(32), base64.b64decode(a["CREWDOCK_MASTER_KEY"]))
            self.assertEqual("owner@example.com", a["CREWDOCK_BOOTSTRAP_EMAIL"])
            self.assertIn(f":{a['POSTGRES_PASSWORD']}@", a["CREWDOCK_DATABASE_URL"])
            for key in ("POSTGRES_PASSWORD", "CREWDOCK_MASTER_KEY", "CREWDOCK_SESSION_SECRET", "CREWDOCK_BOOTSTRAP_PASSWORD"):
                self.assertNotEqual(a[key], b[key])
            original = first.read_bytes()
            retry = subprocess.run([sys.executable, str(SCRIPT), "--output", str(first)], capture_output=True)
            self.assertNotEqual(0, retry.returncode)
            self.assertEqual(original, first.read_bytes())

    def test_email_cannot_inject_another_environment_setting(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "unsafe.env"
            result = subprocess.run([sys.executable, str(SCRIPT), "--output", str(output), "--email", "owner@example.com\nINJECTED=yes"], capture_output=True)
            self.assertNotEqual(0, result.returncode)
            self.assertFalse(output.exists())


if __name__ == "__main__":
    unittest.main()
