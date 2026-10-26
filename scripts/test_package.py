import contextlib
import io
import json
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch

import package


class NotarizationTests(unittest.TestCase):
    @patch("package.subprocess.run")
    def test_rejected_submission_fails_and_fetches_diagnostics(self, run):
        run.return_value = subprocess.CompletedProcess(
            [], 0, json.dumps({"id": "submission-id", "status": "Invalid"}), ""
        )
        with self.assertRaisesRegex(RuntimeError, "Invalid"), contextlib.redirect_stdout(io.StringIO()):
            package.notarize_macos(Path("release.dmg"), "release", "/tmp/signing.keychain-db")
        self.assertEqual(run.call_args_list[-1].args[0][1:4], ["notarytool", "log", "submission-id"])

    @patch("package.subprocess.run")
    def test_authentication_failure_is_reported_and_aborts(self, run):
        run.return_value = subprocess.CompletedProcess([], 1, "", "Invalid credentials")
        errors = io.StringIO()
        with self.assertRaises(subprocess.CalledProcessError), contextlib.redirect_stderr(errors):
            package.notarize_macos(Path("release.dmg"), "release", None)
        self.assertIn("Invalid credentials", errors.getvalue())
        self.assertEqual(run.call_count, 1)

    @patch("package.subprocess.run")
    def test_accepted_submission_succeeds(self, run):
        run.return_value = subprocess.CompletedProcess(
            [], 0, json.dumps({"id": "submission-id", "status": "Accepted"}), ""
        )
        with contextlib.redirect_stdout(io.StringIO()):
            package.notarize_macos(Path("release.dmg"), "release", None)

    def test_notarization_without_signing_is_rejected_before_packaging(self):
        for identity in ([], ["--sign-identity", "-"]):
            with self.subTest(identity=identity), patch("sys.argv", ["package.py", "--notary-profile", "release", *identity]):
                with patch("package.shutil.rmtree") as remove, contextlib.redirect_stderr(io.StringIO()):
                    with self.assertRaises(SystemExit) as error:
                        package.main()
                    self.assertEqual(error.exception.code, 2)
                    remove.assert_not_called()


if __name__ == "__main__":
    unittest.main()
