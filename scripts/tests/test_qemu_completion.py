import importlib.util
from pathlib import Path
import unittest
import enum
import subprocess
import tempfile
import threading
from types import SimpleNamespace

spec = importlib.util.spec_from_file_location("qemu_completion", Path(__file__).parents[1] / "qemu_completion.py")
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)


class CompletionProtocol(unittest.TestCase):
    def test_prompt_and_command_echo_are_not_completion(self):
        output = "/ # \n/ # /test; printf '\\n__CASE__:%s\\n' \"$?\"\n"
        self.assertIsNone(runner.exit_code(output, "__CASE__"))
        self.assertIsNone(runner.exit_code(output + "__PREVIOUS__:0\n", "__CASE__"))
        self.assertEqual(runner.exit_code(output + "__CASE__:0\r\n/ # ", "__CASE__"), 0)

    def test_real_shell_waits_for_exit_instead_of_prompt(self):
        class Status(enum.Enum):
            OK = 0
            FAILED = 1
            TIMEOUT = 2

        class Base:
            def check_output(self, output):
                return Status.OK

        instance = runner.completion_runner(Base, Status)()
        instance.lk = threading.Lock()
        instance.output = ""
        instance.thread_stop_flag = False
        instance.logger = SimpleNamespace(println=lambda message: None, println_file_only=lambda message: None)
        instance.zcore_proc = subprocess.Popen(["sh"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, start_new_session=True)

        def read():
            for line in instance.zcore_proc.stdout:
                with instance.lk:
                    instance.output += line.decode()

        instance.receiver_thread = threading.Thread(target=read)
        instance.receiver_thread.start()
        try:
            with tempfile.TemporaryDirectory() as directory:
                script = Path(directory) / "case.sh"
                script.write_text("#!/bin/sh\nprintf '/ # \\n'\nsleep 0.05\nexit 7\n")
                script.chmod(0o755)
                self.assertEqual(instance.run_one(str(script), timeout=2), Status.FAILED)
                script.write_text("#!/bin/sh\nexit 0\n")
                self.assertEqual(instance.run_one(str(script), timeout=2), Status.OK)
                script.write_text("#!/bin/sh\nsleep 10\n")
                self.assertEqual(instance.run_one(str(script), timeout=0.05), Status.TIMEOUT)
        finally:
            instance.stop_qemu()
            instance.zcore_proc.stdin.close()
            instance.zcore_proc.stdout.close()
        self.assertFalse(instance.receiver_thread.is_alive())

    def test_nonzero_exit_and_incomplete_marker(self):
        self.assertEqual(runner.exit_code("__CASE__:127\n", "__CASE__"), 127)
        self.assertIsNone(runner.exit_code("__CASE__:", "__CASE__"))


if __name__ == "__main__":
    unittest.main()
