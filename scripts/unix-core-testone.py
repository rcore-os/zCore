import pexpect
import sys
import re
import argparse

TIMEOUT = 300
ZIRCON_LOADER_PATH = 'zircon-loader'
PREBUILT_PATH = '../prebuilt/zircon/x64'
CMDLINE_BASE = 'LOG=warn:userboot=test/core-standalone-test:userboot.shutdown:core-tests='

parser = argparse.ArgumentParser()
parser.add_argument('testcase', nargs=1)
args = parser.parse_args()

child = pexpect.spawn("cargo run -p '%s' -- '%s' '%s' --debug " % 
                        (ZIRCON_LOADER_PATH, PREBUILT_PATH, CMDLINE_BASE+args.testcase[0]), 
                        timeout=TIMEOUT, encoding='utf-8')

child.logfile = sys.stdout

index = child.expect(['finished!', 'panicked', pexpect.EOF, pexpect.TIMEOUT])
result = ['FINISHED', 'PANICKED', 'EOF', 'TIMEOUT'][index]
print(result)
