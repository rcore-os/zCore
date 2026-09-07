"""Run Zircon core-tests with explicit skips and separate kernel/guest logs."""
import argparse
import fnmatch
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
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


def check_output(output, expected=()):
    """Require every requested case, a complete summary and successful guest exit."""
    summary = re.search(r"\[==========\] (\d+) tests? from \d+ test cases? ran ", output)
    started = re.findall(r"\[ RUN      \] (\S+)", output)
    completed = [name for name in re.findall(r"\[(?:       OK |  SKIPPED )\] (\S+)", output) if name in started]
    return bool(
        summary and started and int(summary[1]) == len(started)
        and started == completed and not set(expected).difference(started)
        and "[  FAILED  ]" not in output
        and ("*** Exit status 0 ***" in output or "userboot: finished!" in output)
    )


def discovered_tests(output):
    suite = None
    tests = []
    for line in output.splitlines():
        if re.fullmatch(r"[\w/]+", line):
            suite = line
        elif suite and re.fullmatch(r"  \.[\w/]+", line):
            tests.append(suite + line.strip())
    return tests


def load_expectations(arch, libos):
    mode = "libos" if libos else "bare"
    platform = f"{arch}-{mode}"
    path = ROOT / "tests/testcases/zircon_core_test" / f"{platform.replace('-', '_')}.txt"
    if not path.exists():
        path = path.with_name(f"x86_64_{mode}.txt")
    cases = {}
    for line in path.read_text().splitlines():
        fields = line.split()
        if len(fields) == 2 and not line.startswith("#"):
            cases[fields[0]] = None if fields[1] == "OK" else f"Existing classification: {fields[1]}"
    overrides = json.loads((ROOT / "scripts/test_expectations/zircon.json").read_text())
    for pattern in overrides.get("supported", []):
        for name in cases:
            if fnmatch.fnmatchcase(name, pattern):
                cases[name] = None
    for entry in overrides["skips"]:
        if any(fnmatch.fnmatchcase(platform, pattern) for pattern in entry["platforms"]):
            cases[entry["test"]] = entry["reason"]
    return cases


class Runner:
    def __init__(self, args):
        self.args = args
        self.log_dir = Path(args.log_dir or ROOT / "target/test-logs" / f"zircon-{args.arch}-{'libos' if args.libos else 'bare'}").resolve()
        self.log_dir.mkdir(parents=True, exist_ok=True)
        self.results = []
        self.make = ["make", "-C", str(ROOT / "zCore"), "MODE=release", "ZBI=core-tests", "TEST=1", "BOOT_DISK_READONLY=on", f"ARCH={args.arch}"]
        for key, value in [("SMP", args.smp), ("X64_CPU", args.x64_cpu), ("qemu", args.qemu)]:
            if value is not None:
                self.make.append(f"{key}={value}")
        if args.libos:
            self.make.append("LIBOS=1")

    def run(self, selection, expected=()):
        number = len(self.results)
        label = re.sub(r"[^a-zA-Z0-9_.-]", "_", selection)[:100]
        prefix = self.log_dir / f"{number:04}-{label}"
        kernel_log = str(prefix) + ".kernel.log"
        cmdline = "LOG=info:userboot=test/core-standalone-test:userboot.shutdown:core-tests=" + selection
        env = dict(os.environ, ZCORE_KERNEL_LOG=kernel_log)
        if self.args.libos:
            zircon_arch = {"x86_64": "x64", "aarch64": "arm64", "riscv64": "riscv64"}[self.args.arch]
            command = [str(ROOT / "target/release/zcore"), str(ROOT / f"prebuilt/zircon/{zircon_arch}/core-tests.zbi"), cmdline]
        else:
            command = self.make + [f"KERNEL_LOG={kernel_log}", f"CMDLINE={cmdline}", "justrun"]
        start = time.monotonic()
        with open(str(prefix) + ".host.log", "wb") as errors:
            proc = subprocess.Popen(command, cwd=ROOT, env=env, stdout=subprocess.PIPE, stderr=errors, start_new_session=True)
            try:
                limit = max(self.args.timeout, 300) if "PortStressTest." in selection else self.args.timeout
                output, _ = proc.communicate(timeout=limit)
                text = output.decode(errors="replace")
                complete = bool(discovered_tests(text)) and "*** Exit status 0 ***" in text if selection == "-l" else check_output(text, expected)
                status = "OK" if proc.returncode == 0 and complete else "FAILED"
            except subprocess.TimeoutExpired:
                os.killpg(proc.pid, signal.SIGKILL)
                output, _ = proc.communicate()
                status = "TIMEOUT"
        self.output = output.decode(errors="replace")
        Path(str(prefix) + ".guest.log").write_bytes(output)
        record = {"selection": selection, "status": status, "seconds": round(time.monotonic() - start, 3), "returncode": proc.returncode, "passed": len(re.findall(r"\[       OK \]", self.output)) if status == "OK" else 0, "guest_skipped": re.findall(r"\[  SKIPPED \] (\S+\.\S+)", self.output), "log": str(prefix)}
        self.results.append(record)
        print(f"{status}: {selection} ({record['seconds']}s, exit={proc.returncode})", flush=True)
        if status != "OK":
            print(output.decode(errors="replace")[-5000:], flush=True)
            print(f"Diagnostics: {prefix}.kernel.log and {prefix}.host.log", flush=True)
        return status == "OK"

    def finish(self, skipped):
        (self.log_dir / "results.json").write_text(json.dumps({"skipped": skipped, "runs": self.results}, indent=2) + "\n")
        failures = sum(result["status"] != "OK" for result in self.results)
        print(f"{sum(result['passed'] for result in self.results)} tests passed; {len(self.results)} runs, {failures} failed; {len(skipped)} explicitly skipped. Logs: {self.log_dir}")
        return failures == 0 and bool(self.results)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("-l", "--libos", action="store_true")
    parser.add_argument("-a", "--arch", choices=["x86_64", "aarch64", "riscv64"], default="x86_64")
    parser.add_argument("-f", "--fast", action="store_true", help="compatibility option; known failures are explicitly skipped in all CI runs")
    selection = parser.add_mutually_exclusive_group()
    selection.add_argument("-t", "--test", help="comma-separated positive filter")
    selection.add_argument("--group", choices=GROUPS)
    parser.add_argument("--include-skipped", action="store_true", help="opt into known failing or unsupported cases for debugging")
    parser.add_argument("--smp", type=int, choices=range(1, 9))
    parser.add_argument("--skip-build", action="store_true")
    parser.add_argument("--timeout", type=int, default=90)
    parser.add_argument("--x64-cpu")
    parser.add_argument("--qemu")
    parser.add_argument("--log-dir")
    parser.add_argument("--batch-size", type=int, default=8)
    args = parser.parse_args()
    if args.batch_size < 1:
        parser.error("batch size must be positive")
    runner = Runner(args)
    if not args.skip_build:
        subprocess.run(runner.make + ["build"], check=True)
    if not runner.run("-l"):
        runner.finish({})
        return 1
    available = discovered_tests(runner.output)
    expectations = load_expectations(args.arch, args.libos)
    config = json.loads((ROOT / "scripts/test_expectations/zircon.json").read_text())
    selection = args.test or (GROUPS[args.group] if args.group else "*")
    selected, skipped = [], {}
    for name in sorted(available):
        if not any(fnmatch.fnmatchcase(name, pattern) for pattern in selection.split(",")):
            continue
        reason = expectations.get(name, "Not yet classified for this core-tests image")
        if name not in expectations and any(fnmatch.fnmatchcase(name, pattern) for pattern in config.get("supported", [])):
            reason = None
        if reason and not args.include_skipped:
            skipped[name] = reason
            print(f"SKIP: {name}: {reason}")
        else:
            selected.append(name)
    if not selected:
        print("No runnable tests matched the selection", file=sys.stderr)
        runner.finish(skipped)
        return 1
    # Userboot's test filter is bounded. Keep batches below its command-line
    # limit, and verify every exact name to detect truncation or missing cases.
    batch = []
    for name in selected:
        if name.startswith("PortStressTest."):
            if batch:
                runner.run(",".join(batch), expected=batch)
                batch = []
            runner.run(name, expected=[name])
            continue
        if batch and (len(batch) >= args.batch_size or len(",".join(batch + [name])) > 180):
            runner.run(",".join(batch), expected=batch)
            batch = []
        batch.append(name)
    if batch:
        runner.run(",".join(batch), expected=batch)
    return 0 if runner.finish(skipped) else 1


if __name__ == "__main__":
    sys.exit(main())
