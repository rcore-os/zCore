import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("zircon_runner", Path(__file__).parents[1] / "zircon_core_test.py")
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)


class ResultValidation(unittest.TestCase):
    def test_complete_output(self):
        output = "[ RUN      ] Suite.Case\n[       OK ] Suite.Case\n[==========] 1 test from 1 test case ran (0 ms).\n*** Exit status 0 ***"
        self.assertTrue(runner.check_output(output, ["Suite.Case"]))
        self.assertFalse(runner.check_output(output, ["Suite.Missing"]))
        self.assertFalse(runner.check_output(output.replace("*** Exit status 0 ***", "")))
        self.assertFalse(runner.check_output(output + "\n[  FAILED  ] Suite.Case"))

    def test_runtime_skip_is_explicit_and_complete(self):
        output = "[ RUN      ] Suite.Case\n[  SKIPPED ] Suite.Case (0 ms)\n[==========] 1 test from 1 test case ran (0 ms).\n[  SKIPPED ] 1 test\n*** Exit status 0 ***"
        self.assertTrue(runner.check_output(output, ["Suite.Case"]))
        self.assertFalse(runner.check_output(output.replace("[  SKIPPED ] Suite.Case (0 ms)", "")))

    def test_empty_run_is_not_success(self):
        self.assertFalse(runner.check_output("[==========] 0 test from 0 test case ran (0 ms).\n*** Exit status 0 ***"))

    def test_catalog_preserves_parameterized_names(self):
        self.assertEqual(runner.discovered_tests("startup text\nSuite\n  .One\n  .Two\n/Variant\n  .Case/arm64\n"), ["Suite.One", "Suite.Two", "/Variant.Case/arm64"])


if __name__ == "__main__":
    unittest.main()
