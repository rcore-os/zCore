import pexpect
import sys
import re
import os

TIMEOUT = 100
ZCORE_PATH = '../zCore'
BASE = 'zircon/'
OUTPUT_FILE = BASE + 'test-output-baremetal.txt'
RESULT_FILE = BASE + 'test-result-baremetal.txt'
CHECK_FILE = BASE + 'test-check-passed.txt'
TEST_CASE_ALL = BASE + 'testcases-all.txt'
TEST_CASE_EXCEPTION = BASE + 'testcases-failed-baremetal.txt'

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
    child = pexpect.spawn("make -C %s test mode=release test_filter='%s'" %
                            (ZCORE_PATH, line.replace('\n', '')),
                            timeout=TIMEOUT,
                            encoding='utf-8')
    child.logfile = Tee(OUTPUT_FILE, 'a')

    index = child.expect(
        ['finished!', 'panicked', pexpect.EOF, pexpect.TIMEOUT])
    result = ['FINISHED', 'PANICKED', 'EOF', 'TIMEOUT'][index]
    print(result)

passed = []
failed = []
passed_case = set()
failed_case = set()

# see https://stackoverflow.com/questions/59379174/ignore-ansi-colors-in-pexpect-response
ansi_escape = re.compile(r"\x1B[@-_][0-?]*[ -/]*[@-~]")

with open(OUTPUT_FILE, "r") as opf:
    for line in opf:
        line = ansi_escape.sub('', line)
        if line.startswith('[       OK ]'):
            passed += line
            passed_case.add(line[13:].split(' ')[0])
        elif line.startswith('[  FAILED  ]') and line.endswith(')\n'):
            failed += line
            failed_case.add(line[13:].split(' ')[0])

with open(RESULT_FILE, "a") as rstf:
    rstf.writelines(passed)
    rstf.writelines(failed)
# with open(CHECK_FILE, 'r') as f:
#     check_case = set([case.strip() for case in f.readlines()])

# not_passed = check_case - passed_case
if failed_case:
    print('=== Failed cases ===')
    for case in failed_case:
        print(case)
    exit(1)
else:
    print('All checked case passed!')
