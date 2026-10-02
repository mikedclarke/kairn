import importlib.util
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "help_text_eval", Path(__file__).with_name("help-text-eval.py")
)
evaluation = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = evaluation
spec.loader.exec_module(evaluation)


class VaultBoundaryTests(unittest.TestCase):
    def test_root_overrides_never_launch_the_cli(self):
        with tempfile.TemporaryDirectory() as directory:
            scratch = Path(directory) / "scratch"
            outside = Path(directory) / "outside"
            scratch.mkdir()
            outside.mkdir()
            note = outside / "note.md"
            note.write_text("preserve me")
            commands = [
                ["--root", str(outside), "add", "unexpected"],
                ["add", "unexpected", "--root", str(outside)],
                ["add", "unexpected", f"--root={outside}"],
            ]
            with patch.object(evaluation.subprocess, "run") as run:
                for args in commands:
                    code, _, error = evaluation.run_kairn(Path("kairn"), args, scratch)
                    self.assertEqual(code, 2)
                    self.assertIn("owns --root", error)
                run.assert_not_called()
            self.assertEqual(note.read_text(), "preserve me")
            self.assertEqual(list(scratch.iterdir()), [])

    def test_harness_supplies_root_and_keeps_literal_payloads(self):
        with tempfile.TemporaryDirectory() as directory:
            vault = Path(directory)
            with patch.object(evaluation.subprocess, "run") as run:
                run.return_value.returncode = 0
                run.return_value.stdout = "ok"
                run.return_value.stderr = ""
                args = ["capture", "--", "--root=literal text"]
                self.assertEqual(evaluation.run_kairn(Path("/bin/kairn"), args, vault), (0, "ok", ""))
                self.assertEqual(run.call_args.args[0], ["/bin/kairn", "--root", str(vault)] + args)
                self.assertEqual(run.call_args.kwargs["cwd"], vault)


if __name__ == "__main__":
    unittest.main()
