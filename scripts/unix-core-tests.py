import pexpect
import sys
import re
import os

TIMEOUT = 300
ZIRCON_LOADER_PATH = 'zircon-loader'
BASE = 'zircon/'
OUTPUT_FILE = BASE + 'test-output-libos.txt'
RESULT_FILE = BASE + 'test-result-libos.txt'
CHECK_FILE = BASE + 'test-check-passed.txt'
TEST_CASE_ALL = BASE + 'testcases-all.txt'
TEST_CASE_EXCEPTION = BASE + 'testcases-failed-libos.txt'
PREBUILT_PATH = '../prebuilt/zircon/x64'
CMDLINE_BASE = 'LOG=warn:userboot=test/core-standalone-test:userboot.shutdown:core-tests='

class Tee:
    def __init__(self, name, mode):
        self.file = open(name, mode)
        self.stdout = sys.stdout
        sys.stdout = self

    def __del__(self):
        sys.stdout = self.stdout
        self.file.close()

    def write(self, data):
        self.file.write(data)
        self.stdout.write(data)

    def flush(self):
        self.file.flush()

if os.path.exists(OUTPUT_FILE): os.remove(OUTPUT_FILE)
if os.path.exists(RESULT_FILE): os.remove(RESULT_FILE)

with open(TEST_CASE_ALL, "r") as tcf:
    all_case = set([case.strip() for case in tcf.readlines()])
with open(TEST_CASE_EXCEPTION, "r") as tcf:
    exception_case = set([case.strip() for case in tcf.readlines()])
check_case = all_case - exception_case

for line in check_case: 
    child = pexpect.spawn("cargo run -p '%s' -- '%s' '%s' --debug" % 
                    (ZIRCON_LOADER_PATH, PREBUILT_PATH, CMDLINE_BASE+line), 
                    timeout=TIMEOUT, encoding='utf-8')

    child.logfile = Tee(OUTPUT_FILE, 'a')

    index = child.expect(['finished!', 'panicked', pexpect.EOF, pexpect.TIMEOUT])
    result = ['FINISHED', 'PANICKED', 'EOF', 'TIMEOUT'][index]
    # print(result)

passed = []
failed = []
passed_case = set()

# see https://stackoverflow.com/questions/59379174/ignore-ansi-colors-in-pexpect-response
ansi_escape = re.compile(r"\x1B[@-_][0-?]*[ -/]*[@-~]")


with open(OUTPUT_FILE, "r") as opf:
    for line in opf.readlines():
        line=ansi_escape.sub('',line)
        if line.startswith('[       OK ]'):
            passed += line
            passed_case.add(line[13:].split(' ')[0])
        elif line.startswith('[  FAILED  ]') and line.endswith(')\n'):
            failed += line

with open(RESULT_FILE, "a") as rstf:
    rstf.writelines(passed)
    rstf.writelines(failed)


not_passed = check_case - passed_case
if failed:
    print('=== Failed cases ===')
    for case in failed:
        print(case)
    exit(1)
else:
    print('All checked case passed!')
