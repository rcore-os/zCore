import re

BASE = 'zircon/'
OUTPUT_FILE = BASE + 'test-output.txt'
STATISTIC_FILE = BASE + 'test-statistic.txt'


def match():
    ansi_escape = re.compile(r"\x1B[@-_][0-?]*[ -/]*[@-~]")
    need_to_fix_dic = {}
    recording = False
    l = []
    key = ""
    with open(OUTPUT_FILE, "r") as f:
        for line in f.readlines():
            line=ansi_escape.sub('',line)
            if line.startswith('[ RUN      ]') and not recording:
                recording = True
                key = line[13:].split(' ')[0].strip()
                l = []
                l.append(line)
            elif line.startswith('[       OK ]') and line.endswith(')\n'):
                recording = False
                l.append(line)
                # 可写入 另一个 文件
                l = []
            elif line.startswith('[  FAILED  ]') and line.endswith(')\n'):
                recording = False
                l.append(line)
                need_to_fix_dic[key] = l
            elif line.startswith('[ RUN      ]') and recording:
                need_to_fix_dic[key] = l
                key = line[13:].split(' ')[0].strip()
                l =[]
                l.append(line)
            elif recording == True:
                l.append(line)

    with open(STATISTIC_FILE, "w") as f:
        index = 0
        for k in need_to_fix_dic.keys() :
            index += 1
            f.write("{0} ============================== {1} ==============================\n".format(index,k))
            f.writelines(need_to_fix_dic[k])
            f.write("============================== End ==============================\n")
            f.write("\n\n")

match()