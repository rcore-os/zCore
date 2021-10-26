import pexpect
import sys
import re
import argparse

TIMEOUT = 300
ZBI_PATH = '../prebuilt/zircon/x64/core-tests.zbi'
CMDLINE_BASE = 'LOG=warn:userboot=test/core-standalone-test:userboot.shutdown:core-tests='

parser = argparse.ArgumentParser()
parser.add_argument('testcase', nargs=1)
args = parser.parse_args()

child = pexpect.spawn("cargo run -p zcore --release --features 'zircon libos' -- '%s' '%s'" %
                        (ZBI_PATH, CMDLINE_BASE+args.testcase[0]),
                        timeout=TIMEOUT, encoding='utf-8')

child.logfile = sys.stdout

index = child.expect(['finished!', 'panicked', pexpect.EOF, pexpect.TIMEOUT])
result = ['FINISHED', 'PANICKED', 'EOF', 'TIMEOUT'][index]
print(result)
