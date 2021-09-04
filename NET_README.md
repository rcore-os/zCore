# Net support 

## how to run

项目根目录下
```
cd zCore 
make run mode=release linux=1
```
进入 内核
```
wget 10.0.2.2
```
(前提 是 10.0.2.2 运行有一个 服务端 默认 端口80)
10.0.2.2 是qemu 上 运行 的 OS 看到 的 地址 、映射的 是 host 主机 的 地址

目前wget 外网 地址不通、不清楚原因

## 快速 运行 一个 nginx （ubuntu 下）

```
// 安装 nginx
sudo apt install nginx
// 验证安装
nginx -v
// 启动 nginx
service nginx start
```
