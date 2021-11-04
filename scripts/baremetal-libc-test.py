import os
import glob
import subprocess
import re
import sys
# ===============Must Config========================

TIMEOUT = 10  # seconds
ZCORE_PATH = '../zCore'
BASE = 'linux/'
CHECK_FILE = BASE + 'baremetal-test-allow.txt'
FAIL_FILE = BASE + 'baremetal-test-fail.txt'
RBOOT_FILE = 'rboot.conf'
RESULT_FILE ='../stdout-zcore'
rboot= r'''
# The config file for rboot.
# Place me at \EFI\Boot\rboot.conf

# The address at which the kernel stack is placed.
# kernel_stack_address=0xFFFFFF8000000000

# The size of the kernel stack, given in number of 4KiB pages. Defaults to 512.
# kernel_stack_size=128

# The virtual address offset from which physical memory is mapped, as described in
# https://os.phil-opp.com/paging-implementation/#map-the-complete-physical-memory
physical_memory_offset=0xFFFF800000000000

# The path of kernel ELF
kernel_path=\EFI\zCore\zcore.elf

# The resolution of graphic output
resolution=1024x768

initramfs=\EFI\zCore\x86_64.img
# LOG=debug/info/error/warn/trace
# add ROOTPROC info  ? split CMD and ARG : ROOTPROC=/libc-test/src/functional/argv.exe?   OR ROOTPROC=/bin/busybox?sh
cmdline=LOG=error:TERM=xterm-256color:console.shell=true:virtcon.disable=true:ROOTPROC='''

# ==============================================
passed = set()
failed = set()
timeout = set()

FAILED = [
    "failed",
    "ERROR",
]

with open(CHECK_FILE, 'r') as f:
    allow_files = set([case.strip() for case in f.readlines()])

with open(FAIL_FILE,'r') as f:
    failed_files = set([case.strip() for case in f.readlines()])

for file in allow_files:
    if not (file in failed_files):
#        print(file)
        rboot_file=rboot+file+'?'
#        print(rboot)
        with open(RBOOT_FILE,'w') as f:
            print(rboot_file, file=f)
        try:
            subprocess.run(r'cp rboot.conf ../zCore && cd ../ && make baremetal-test | tee stdout-zcore '
                           r'&& '
                           r'sed -i '
                           r'"/BdsDxe/d" stdout-zcore',
                           shell=True, timeout=TIMEOUT, check=True)

            with open(RESULT_FILE, 'r') as f:
                output=f.read()

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
print("Total tested num: ", len(allow_files)-len(failed_files))
print("=======================================")
# with open(FAIL_FILE,'w') as f:
#     for bad_file in failed:
#         print(bad_file, file=f)

if len(failed) > 3 :
    sys.exit(-1)
else:
    sys.exit(0)
