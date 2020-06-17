import pexpect

ZCORE_PATH = '../zCore'
OUTPUT_FILE = 'test-output.txt'
RESULT_FILE = 'test-result.txt'
TEST_CASE_FILE = 'testcases.txt'

with open(TEST_CASE_FILE, "r") as f:
    lines = f.readlines()
    positive = [line for line in lines if not line.startswith('-')]
    negative = [line[1:] for line in lines if line.startswith('-')]
    test_filter = (','.join(positive) + '-' + ','.join(negative)).replace('\n', '')

child = pexpect.spawn("make -C %s test mode=release accel=1 test_filter='%s'" % (ZCORE_PATH, test_filter), timeout=120)
child.logfile = open(OUTPUT_FILE, "wb")

index = child.expect(['finished!', 'panicked', pexpect.EOF, pexpect.TIMEOUT])
result = ['FINISHED', 'PANICKED', 'EOF', 'TIMEOUT'][index]
print(result)

passed = []
failed = []
with open(OUTPUT_FILE, "r") as f:
    for line in f.readlines():
        if line.startswith('[       OK ]'):
            passed += line
        elif line.startswith('[  FAILED  ]'):
            failed += line

with open(RESULT_FILE, "w") as f:
    f.writelines(passed)
    f.writelines(failed)
