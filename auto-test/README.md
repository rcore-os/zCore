# Easy work with test

简单 测试

## Require Environment

- 继承zCore的运行环境
- python3.8 [yes 3.8]
- pip3

## Install Environment

下载 所需的pexpect
```
pip3 install pexpect
```

## How to use

### Step1 开启测试所需

当你第一次 clone 下来 zcore后、[设:环境ok]

1. 配置你的core-tests在那里
2. 修改 zCore文件目录下 rboot.conf 里 被注释的第23行、解注释、22行、注释
3. 进入 zircon-syscall/src/channel.rs 中 修改第12行 false 改为 true 开启测试[默认关闭、因为不知道会不会与别的造成影响]
4. 编译程序 
```
make build zbi_file=core-tests
```
如果一切都 ok 进入下一步、不ok 就是上述 有条件不满足、玄学问题请致电微信群zcore

### Step2 开始测试

1. 在auto-test目录下 打开 console 输入

```
sh local_start.sh
```

观察 控制台 应有提示 
```
 等待执行测试....
 在 auto-test/result/result.txt 可实时查看结果
```

2. 看result文件

### Step3 自己配置要测试的测试用例

在 auto-test/work/ 目录下  修改temp-test.json 里的内容、应该一看就懂

core-tests.json 为了写出所有测例的名字 [未完成]

