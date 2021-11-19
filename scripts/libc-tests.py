import os
import glob
import subprocess

# ===============Must Config========================

TIMEOUT = 5  # seconds
ZCORE_PATH = '../zCore'
BASE = 'linux/'
OUTPUT_FILE = BASE + 'test-output.txt'
RESULT_FILE = BASE + 'test-result.txt'
CHECK_FILE = BASE + 'test-allow-failed.txt'

# ==============================================

passed = set()
failed = set()
timeout = set()

for path in glob.glob("../rootfs/libc-test/src/*/*.exe"):
    path = path[len('../rootfs'):]
    # ignore static linked tests
    if path.endswith('-static.exe'):
        continue
    try:
        subprocess.run("cd .. && cargo run --release -p linux-loader -- " + path,
                       shell=True, timeout=TIMEOUT, check=True)
        passed.add(path)
    except subprocess.CalledProcessError:
        failed.add(path)
    except subprocess.TimeoutExpired:
        timeout.add(path)

with open(RESULT_FILE, "w") as f:
    print('PASSED:', file=f)
    for case in passed:
        print(case, file=f)
    print('FAILED:', file=f)
    for case in failed:
        print(case, file=f)
    print('TIMEOUT:', file=f)
    for case in timeout:
        print(case, file=f)

with open(CHECK_FILE, 'r') as f:
    allow_failed = set([case.strip() for case in f.readlines()])

check_failed = (failed | timeout) - allow_failed
if check_failed:
    print('=== Failed cases ===')
    for case in check_failed:
        print(case)
    exit(1)
else:
    print('All checked case passed!')

os.system('killall linux-loader')
