import glob
import subprocess

# ===============Must Config========================

TIMEOUT = 10  # seconds
ZCORE_PATH = '../zCore'
BASE = 'linux/'
OUTPUT_FILE = BASE + 'test-output.txt'
RESULT_FILE = BASE + 'test-result.txt'
CHECK_FILE = BASE + 'test-check-passed.txt'

# ==============================================

for path in glob.glob("../rootfs/libc-test/src/*/*.exe"):
    path = path[9:]
    print('testing', path, end='\t')
    try:
        subprocess.run("cd .. && cargo run --release -p linux-loader " + path,
                       shell=True, timeout=TIMEOUT, check=True)
        print('PASS')
    except subprocess.CalledProcessError:
        print('FAILED')
    except subprocess.TimeoutExpired:
        print('TIMEOUT')
