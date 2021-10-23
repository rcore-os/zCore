import pexpect
import sys
import re

TIMEOUT = 300
ZCORE_PATH = '../zCore'
BASE = 'zircon/'
OUTPUT_FILE = BASE + 'test-output.txt'
RESULT_FILE = BASE + 'test-result.txt'
CHECK_FILE = BASE + 'test-check-passed.txt'
TEST_CASE_FILE = BASE + 'testcases.txt'

CMDLINE = "LOG=warn:userboot=test/core-standalone-test:userboot.shutdown:core-tests=%s"

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


with open(TEST_CASE_FILE, "r") as f:
    lines = f.readlines()
    positive = [line for line in lines if not line.startswith('-')]
    negative = [line[1:] for line in lines if line.startswith('-')]
    test_filter = (','.join(positive) + ((',-' + ','.join(negative) if len(negative) > 0 else "") )).replace('\n', '')

child = pexpect.spawn("make -C %s run MODE=release ZBI=core-tests CMDLINE='%s'" % (ZCORE_PATH, CMDLINE % test_filter),
                      timeout=TIMEOUT, encoding='utf-8')
child.logfile = Tee(OUTPUT_FILE, 'w')

index = child.expect(['finished!', 'panicked', pexpect.EOF, pexpect.TIMEOUT])
result = ['FINISHED', 'PANICKED', 'EOF', 'TIMEOUT'][index]
print(result)

passed = []
failed = []
passed_case = set()

# see https://stackoverflow.com/questions/59379174/ignore-ansi-colors-in-pexpect-response
ansi_escape = re.compile(r"\x1B[@-_][0-?]*[ -/]*[@-~]")

with open(OUTPUT_FILE, "r") as f:
    for line in f.readlines():
        line=ansi_escape.sub('',line)
        if line.startswith('[       OK ]'):
            passed += line
            passed_case.add(line[13:].split(' ')[0])
        elif line.startswith('[  FAILED  ]') and line.endswith(')\n'):
            failed += line

with open(RESULT_FILE, "w") as f:
    f.writelines(passed)
    f.writelines(failed)

with open(CHECK_FILE, 'r') as f:
    check_case = set([case.strip() for case in f.readlines()])

not_passed = check_case - passed_case
if not_passed:
    print('=== Failed cases ===')
    for case in not_passed:
        print(case)
    exit(1)
else:
    print('All checked case passed!')
