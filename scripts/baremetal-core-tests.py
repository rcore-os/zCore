import pexpect
import sys
import re
import os

TIMEOUT = 20
ZCORE_PATH = '../zCore'
BASE = 'zircon/'
OUTPUT_FILE = BASE + 'test-output-baremetal.txt'
RESULT_FILE = BASE + 'test-result-baremetal.txt'
CHECK_FILE = BASE + 'test-check-passed.txt'
DBG_FILE = BASE + 'dbg-b.txt'
TEST_CASE_FILE = BASE + 'testcases-all.txt'


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
if os.path.exists(DBG_FILE): os.remove(DBG_FILE)

with open(TEST_CASE_FILE, "r") as tcf:
    lines = tcf.readlines()
    for line in lines:
        with open(DBG_FILE, "a") as dbg: print(line, file=dbg)

        child = pexpect.spawn("make -C %s test MODE=release TEST_FILTER='%s'" % (ZCORE_PATH, line.replace('\n','')),
                            timeout=TIMEOUT, encoding='utf-8')
        child.logfile = Tee(OUTPUT_FILE, 'a')

        index = child.expect(['finished!', 'panicked', pexpect.EOF, pexpect.TIMEOUT])
        result = ['FINISHED', 'PANICKED', 'EOF', 'TIMEOUT'][index]
        print(result)

passed = []
failed = []
passed_case = set()

# see https://stackoverflow.com/questions/59379174/ignore-ansi-colors-in-pexpect-response
ansi_escape = re.compile(r"\x1B[@-_][0-?]*[ -/]*[@-~]")

with open(OUTPUT_FILE, "r") as opf:
    for line in opf:
        line=ansi_escape.sub('',line)
        with open(RESULT_FILE, "a") as rstf:
            if line.startswith('[       OK ]'):
                print(line, file=rstf)
            elif line.startswith('[  FAILED  ]') and line.endswith(')\n'):
                print(line, file=rstf)


# with open(CHECK_FILE, 'r') as f:
#     check_case = set([case.strip() for case in f.readlines()])

# not_passed = check_case - passed_case
# if not_passed:
#     print('=== Failed cases ===')
#     for case in not_passed:
#         print(case)
#     exit(1)
# else:
#     print('All checked case passed!')
