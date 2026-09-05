import sys
import argparse
import os
import re
import shlex
from pathlib import Path

# Reuse the existing test runner without requiring callers to change directory.
ROOT = Path(__file__).resolve().parents[1]
os.chdir(ROOT / "tests")
sys.path.insert(0, str(ROOT / "tests"))
from utils.log import Logger
from utils.test import TestRunner, TestStatus, load_testcases

parser = argparse.ArgumentParser()
parser.add_argument("-l", "--libos", action="store_true", help="test on libos mode (otherwise bare-metal mode)")
parser.add_argument("-a", "--arch", choices=["x86_64", "aarch64", "riscv64"], default="x86_64", help="target architecture")
parser.add_argument("-f", "--fast", action="store_true", help="do not test known failed and timeout testcases")
selection = parser.add_mutually_exclusive_group()
selection.add_argument("-t", "--test", help="run a test name or comma-separated positive filter")
selection.add_argument("--group", choices=["ipc-port", "port-stress"], help="run a reproducible regression group")
parser.add_argument("--smp", type=int, choices=range(1, 9), help="bare-metal CPU count (x64 defaults to 4)")
parser.add_argument("--skip-build", action="store_true", help="reuse an already built and packaged kernel")
parser.add_argument("--timeout", type=int, default=90, help="timeout per boot in seconds (default: 90)")
parser.add_argument("--x64-cpu", help="QEMU x64 CPU, e.g. Haswell,+smap,+fsgsbase,-x2apic")
parser.add_argument("--qemu", help="path to the target architecture's QEMU executable")
parser.add_argument("--no-failed", action="store_true", help="exit with calling exit(0), never call exit(-1)")
args = parser.parse_args()


ZIRCON_ARCH = {
    "x86_64": "x64",
    "aarch64": "arm64",
    "riscv64": "riscv64",
}[args.arch]
ZBI_PATH = "../prebuilt/zircon/%s/core-tests.zbi" % ZIRCON_ARCH
TEST_DIR = "testcases/zircon_core_test"
TEST_NAME = "%s_%s" % (args.arch, "libos" if args.libos else "bare")
TEST_FILE = "%s/%s.txt" % (TEST_DIR, TEST_NAME)
if not os.path.exists(TEST_FILE):
    # Until an architecture gets its own classification, use the common x64
    # expectations. CI output will identify cases that need arch-specific
    # classification without duplicating a large generated test list.
    TEST_FILE = "%s/x86_64_%s.txt" % (
        TEST_DIR,
        "libos" if args.libos else "bare",
    )
LOG_OUTPUT = "zircon_core_test_%s.log" % TEST_NAME

TIMEOUT = args.timeout
GROUPS = {
    "ipc-port": ",".join([
        "ChannelCallEtcTest.*", "ChannelWriteEtcTest.*", "IOVecTest.*", "FifoTest.*",
        "SocketTest.*", "StreamTestCase.*", "TimerTest.*", "PortTest.PortTimeout",
        "PortTest.AsyncWait*", "PortTest.Event*", "PortTest.Channel*", "PortTest.Cancel*",
        "PortTest.ThreadEvents", "PortTest.Timestamp", "PortTest.Edge*", "PortTest.Create*",
        "PortTest.Wait*", "PortTest.QueueWaitVerifyUserPacket", "PortTest.QueueNullPtrReturnsInvalidArgs",
        "PortTest.QueueAndClose", "PortTest.QueueWrongType", "PortTest.QueueAccessDenied",
    ]),
    "port-stress": ",".join("PortStressTest." + name for name in [
        "CancelKeyDuringMatchRace", "CancelKeyActiveObserverRace", "CancelKeySharedKeyRace",
        "CancelKeyDuringRegistrationRace", "QueuePacketAfterPortClosedConcurrentRace",
        "CancelKeyDestructorReentersPortLock",
    ]),
}
CMDLINE_BASE = "LOG=error:userboot=test/core-standalone-test:userboot.shutdown:core-tests="
FAILED_PATTERN = [
    "[  FAILED  ]",
    "ERROR",
]


class ZirconTestRunner(TestRunner):
    BASE_CMD = "cd ../zCore && make MODE=release ZBI=core-tests TEST=1 BOOT_DISK_READONLY=on ARCH=%s" % args.arch
    for key, value in [("SMP", args.smp), ("X64_CPU", args.x64_cpu), ("qemu", args.qemu)]:
        if value is not None:
            BASE_CMD += " " + shlex.quote(key + "=" + str(value))

    def build_cmdline(self) -> str:
        return self.BASE_CMD + (" LIBOS=1" if args.libos else "")

    def run_cmdline(self, name: str) -> str:
        if args.libos:
            return "../target/release/zcore %s %s" % (shlex.quote(ZBI_PATH), shlex.quote(CMDLINE_BASE + name))
        else:
            return self.BASE_CMD + " " + shlex.quote("CMDLINE=" + CMDLINE_BASE + name) + " justrun"

    def check_output(self, output: str) -> TestStatus:
        # The current userboot requests a platform shutdown after reporting a
        # boot test. Ignore the unimplemented power-control diagnostic here;
        # the complete test summary and successful guest exit are checked below.
        checked_output = "\n".join(
            line
            for line in output.splitlines()
            if not (
                "syscall unimplemented: SYSTEM_POWERCTL" in line
            )
        )
        for pattern in FAILED_PATTERN:
            if pattern in checked_output:
                return TestStatus.FAILED
        summary = re.search(r"\[==========\] (\d+) tests? from \d+ test cases? ran ", checked_output)
        started = re.findall(r"\[ RUN      \] (\S+)", checked_output)
        passed = re.findall(r"\[       OK \] (\S+)", checked_output)
        if not summary or not started or int(summary[1]) != len(started) or started != passed:
            return TestStatus.FAILED
        if "*** Exit status 0 ***" not in output and "userboot: finished!" not in output:
            return TestStatus.FAILED
        return TestStatus.OK


if __name__ == "__main__":
    runner = ZirconTestRunner()
    if not args.skip_build:
        runner.build()

    if args.test or args.group:
        runner.set_logger(Logger(LOG_OUTPUT))
        res = runner.run_one(GROUPS[args.group] if args.group else args.test, args.fast, TIMEOUT)
        ok = res == TestStatus.OK
    else:
        runner.set_logger(Logger(LOG_OUTPUT))
        testcases = load_testcases(TEST_FILE)
        ok = runner.run_all(testcases, args.fast, TIMEOUT)

    if not ok and not args.no_failed:
        sys.exit(-1)
    else:
        sys.exit(0)
