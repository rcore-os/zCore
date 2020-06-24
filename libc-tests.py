import pexpect

#===============Must Config========================

TIMEOUT = 10 # 秒
ZCORE_PATH = '../zCore'
BASE = 'scripts/linux/'
OUTPUT_FILE = BASE + 'test-output.txt'
RESULT_FILE = BASE + 'test-result.txt'
CHECK_FILE = BASE + 'test-check-passed.txt'
TEST_CASE_FILE = BASE + 'testcases.txt'

#==============================================

# qeme 的 参数

def main():
    print("等待执行测试....\n在scripts/linux/test-result.txt 可实时查看结果")

    lines = []

    with open(TEST_CASE_FILE, "r") as f:
        lines = f.readlines()

    log = open(OUTPUT_FILE,"wb")

    with open(RESULT_FILE,"w",encoding='utf-8',errors='ignore'):
        pass
    
    num = 0

    for line in lines:
        
        if line == '\n' or line.startswith('#'):
            continue

        num+=1
        
        child = pexpect.spawn("cargo run --release -p linux-loader "+line,timeout=TIMEOUT,logfile=log)
        
        name = str(num)+" "+line
        
        index = child.expect([pexpect.EOF,"panicked",pexpect.TIMEOUT]) 
        if index == 0: 
            with open(RESULT_FILE,"a",encoding='utf-8',errors='ignore') as f:
                f.writelines(name+" successed\n")
        elif index == 1:
            with open(RESULT_FILE,"a",encoding='utf-8',errors='ignore') as f:
                f.writelines(name+" failed\n")
        elif index == 2:
            with open(RESULT_FILE,"a",encoding='utf-8',errors='ignore') as f:
                f.writelines(name+" 10s_timeout\n")

if __name__ == "__main__":
    main()

