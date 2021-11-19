import os
import glob
import subprocess
import re
import sys

# ===============Must Config========================

TIMEOUT = 10  # seconds
ZCORE_PATH = '../zCore'
BASE = 'linux/'
CHECK_FILE = BASE + 'baremetal-test-ones-rv64.txt'
SCRIPT_FILE = 'script.sh'
RESULT_FILE ='../stdout-rv64'
script=r'''
#!/bin/bash

cd .. && make baremetal-test-rv64 ROOTPROC='''

# ==============================================
passed = set()
failed = set()
timeout = set()

FAILED = [
    "failed",
    "panicked at",
    "ERROR",
]

with open(CHECK_FILE, 'r') as f:
    allow_files = set([case.strip() for case in f.readlines()])

for file in allow_files:
    script_file = script+file
    with open(SCRIPT_FILE, 'w') as f:
        print(script_file, file=f)
    try:
        subprocess.run(['sh',SCRIPT_FILE], timeout=TIMEOUT, check=True)

        with open(RESULT_FILE, 'r') as f:
            output = f.read()

        break_out_flag = False
        for pattern in FAILED:
            if re.search(pattern, output):
                failed.add(file)
                break_out_flag = True
                break

        if not break_out_flag:
            passed.add(file)
    except subprocess.CalledProcessError:
        failed.add(file)
    except subprocess.TimeoutExpired:
        timeout.add(file)

print("=======================================")
print("PASSED num: ", len(passed))
print("=======================================")
print("FAILED num: ", len(failed))
print(failed)
print("=======================================")
print("TIMEOUT num: ", len(timeout))
print(timeout)
print("=======================================")
