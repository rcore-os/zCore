import os
import time
import glob
import subprocess
from termcolor import colored

# ===============Must Config========================

TIMEOUT = 10  # seconds
ZCORE_PATH = '../zCore'
BASE = 'linux/'
OUTPUT_FILE = BASE + 'test-output.txt'
RESULT_FILE = BASE + 'test-result.txt'
CHECK_FILE = BASE + 'libos-test-allow-failed.txt'

# ==============================================

passed = set()
failed = set()
timeout = set()


def print_cases(cases, file=None):
    for case in sorted(cases):
        print(case, file=file)


subprocess.run("cd .. && cargo build -p zcore --release --features 'libos linux'",
               shell=True, check=True)

for path in sorted(glob.glob("../rootfs/libc-test/src/*/*.exe")):
    path = path[len('../rootfs'):]
    # ignore static linked tests
    if path.endswith('-static.exe'):
        continue
    try:
        time_start = time.time()
        subprocess.run("cd .. && ./target/release/zcore " + path,
                       shell=True, timeout=TIMEOUT, check=True)
        time_end = time.time()
        passed.add(path)
        print(colored('PASSED in %.3fs: %s' % (time_end - time_start, path), 'green'))
    except subprocess.CalledProcessError:
        failed.add(path)
        print(colored('FAILED: %s' % path, 'red'))
    except subprocess.TimeoutExpired:
        timeout.add(path)
        print(colored('TIMEOUT: %s' % path, 'yellow'))

with open(RESULT_FILE, "w") as f:
    print('PASSED:', file=f)
    print_cases(passed, file=f)
    print('FAILED:', file=f)
    print_cases(failed, file=f)
    print('TIMEOUT:', file=f)
    print_cases(timeout, file=f)

with open(CHECK_FILE, 'r') as f:
    allow_failed = set([case.strip() for case in f.readlines()])

more_passed = passed & allow_failed
if more_passed:
    print(colored('=== Passed more cases ===', 'green'))
    print_cases(more_passed)

check_failed = (failed | timeout) - allow_failed
if check_failed:
    print(colored('=== Failed cases ===', 'red'))
    print_cases(failed - allow_failed)
    print(colored('=== Timeout cases ===', 'yellow'))
    print_cases(timeout - allow_failed)
    exit(1)
else:
    print(colored('All checked case passed!', 'green'))

os.system('killall linux-loader')
