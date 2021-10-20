import serial
import os
import sys
import re
import time
import threading
import subprocess

BASE = 'linux/'
CHECK_FILE = BASE + 'baremetal-test-ones.txt'
OUTPUT_FILE = BASE + 'stdout-zcore'
OUTPUT_NET = BASE + 'netout-zcore'
TMP_FILE = BASE + 'tmp-zcore'
TIMEOUT = 60
FAILED = ["failed","ERROR","panicked"]
passed = set()
failed = set()
timeout = set()


def rcv_data():
    while True:
        rcv=serial.readline()
        rcv=rcv.decode() 
        #print(rcv)
        with open(OUTPUT_FILE, 'a') as f: print(rcv, file=f)
        with open(TMP_FILE, 'a') as f: print(rcv, file=f)

def rcv_netdata():
    print("in rcv_netdata")
    with open(OUTPUT_NET, 'w') as f:
        subprocess.run(['tcpdump -en#XXvv'], shell=True, stdout=f)

def net_test():
    print("in net_test")
    # ICMP 
    subprocess.run(['ping 192.168.0.123 -c 4'], shell=True)
    start_time = time.time()
    with open(OUTPUT_NET, 'r') as f: output = f.read()
    if re.search("ICMP", output): passed.add("nettest : ping")
    else: failed.add("nettest : ping")
    # TCP
    try: subprocess.run(['nc -v 192.168.0.123 80'], shell=True, timeout=5)
    except Exception: pass
    start_time = time.time()
    with open(OUTPUT_NET, 'r') as f: output = f.read()
    if re.search("Hello! zCore", output): passed.add("nettest : tcp")
    if time.time() - start_time > 10: timeout.add("nettest : tcp")
    # UDP
    try: subprocess.run(['nc -uv 192.168.0.123 6969'], shell=True, timeout=5)
    except Exception: pass
    start_time = time.time()
    with open(OUTPUT_NET, 'r') as f: output = f.read()
    if re.search("from", output): passed.add("nettest : udp")
    if time.time() - start_time > 10: timeout.add("nettest : udp")

if __name__=='__main__':
#    port_list = list(serial.tools.list_ports.comports())
#    k=0
#    for i in port_list:
#        print(i,k)
#        k=k+1
#
#    if len(port_list) <= 0:
#        print("not find serial")
#        sys.exit()
#
#    serial_k=input("please switch serial:")
#    k = int(serial_k)
#    serial_list = list(port_list[k])
#    serialName = serial_list[0]
    serialName = input("please input serial dev : ")
    serial=serial.Serial(serialName,115200,timeout=3600)

    
    if not serial.isOpen():
        print("open failed >",serial.name)
        with open(OUTPUT_FILE, 'w') as f: print("open failed >", serial.name, file=f)
        sys.exit()

    print("open succeed >",serial.name)
    with open(OUTPUT_FILE, 'w') as f: print("open succeed >", serial.name, file=f)

    th=threading.Thread(target=rcv_data)
    th.setDaemon(True)
    th.start()

    nd = threading.Thread(target=rcv_netdata)
    nd.setDaemon(True)
    nd.start()
    net_test()
    
    print("=======================================")
    print("PASSED num: ", len(passed))
    print("=======================================")
    print("FAILED num: ", len(failed))
    if len(failed) > 0: print(failed)
    print("=======================================")
    print("TIMEOUT num: ", len(timeout))
    if len(timeout) > 0: print(timeout)
    print("=======================================")
    print("Total tested num: 3") 
    print("=======================================")
