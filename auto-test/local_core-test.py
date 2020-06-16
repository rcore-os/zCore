import pexpect
import json
import fileinput

#===============Must Config========================

BaseFile = "../"

BIOS_UEFI = BaseFile + "rboot/OVMF.fd"

EFI_System_Partition = BaseFile + "zCore/target/x86_64/debug/esp"

QEMU_DISK = BaseFile + "zCore/target/x86_64/debug/disk.qcow2"

LogFile = BaseFile + "auto-test/result/logfile.txt"

Result = BaseFile + "auto-test/result/result.txt"

Config = BaseFile + "auto-test/work/temp-test.json"

rbootconf = BaseFile + "zCore/target/x86_64/debug/esp/EFI/boot/rboot.conf"

#==============================================

# qeme 的 参数

# args=["-smp","cores=4","-bios",BIOS_UEFI,"-enable-kvm","-drive","format=raw,file=fat:rw:"+ EFI_System_Partition,
#                                 "-serial","mon:stdio","-m","4G",
#                                 "-device","isa-debug-exit","-drive","format=qcow2,file="+FileSystem+",media=disk,cache=writeback,id=sfsimg,if=none",
#                                 "-device","ahci,id=ahci0","-device","ide-drive,drive=sfsimg,bus=ahci0.0","-nographic"]


args=["-smp","cores=1",
        "-machine","q35",
        "-cpu","Haswell,+smap,-check,-fsgsbase",
        "-bios",BIOS_UEFI,
        "-serial","mon:stdio","-m","4G","-nic","none",
        "-drive","format=raw,file=fat:rw:"+ EFI_System_Partition,
        "-drive","format=qcow2,file="+QEMU_DISK+",id=disk,if=none",
        "-device","ich9-ahci,id=ahci",
        "-device","ide-drive,drive=disk,bus=ahci.0",
        "-device","isa-debug-exit,iobase=0xf4,iosize=0x04",
        "-accel","kvm","-cpu","host,migratable=no,+invtsc"]

def run():

    return 

def main():
    print("等待执行测试....\n在 auto-test/result/result.txt 可实时查看结果")
    
    with open(Config, "r") as f:
        jsondata = f.read()
    data = json.loads(jsondata)

    logfile = open(LogFile,"wb")
    with open(Result,"w",encoding='utf-8',errors='ignore'):
        pass



    for o in data["CoreTests"]:
        name = o["TestCaseName"]
        array = o["TestArray"]
        for i in array: 
            for line in fileinput.input(rbootconf,inplace=1):
                if "#" in line:
                    continue
                if " " in line:
                    continue
                if line.startswith("cmdline"):
                    l = line.split(":")
                    index = len(l)-2
                    l[index] = name+"."+i
                    l.pop()
                    for s in l:
                        print(s,end=":")
                    continue
                print(line,end="")
            fileinput.close()
                
            child = pexpect.spawn("qemu-system-x86_64",args,timeout=10)
            child.logfile = logfile

            index = child.expect(["finished!",pexpect.EOF,pexpect.TIMEOUT]) 
            if index == 0: 
                with open(Result,"a",encoding='utf-8',errors='ignore') as f:
                    f.writelines(name+"."+i+": successed\n")
            elif index == 1:
                with open(Result,"a",encoding='utf-8',errors='ignore') as f:
                    f.writelines(name+"."+i+": EOF\n")
            elif index == 2:
                with open(Result,"a",encoding='utf-8',errors='ignore') as f:
                    f.writelines(name+"."+i+": TIMEOUT\n")


if __name__ == "__main__":
    main()

