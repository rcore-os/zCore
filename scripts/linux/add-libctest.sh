# 下载 libc-test 到 prebuilt/linux/libc-test目录下
echo 正在下载...
git clone git://repo.or.cz/libc-test ../../prebuilt/linux/libc-test || echo 已经下载完成

# 创建文件夹 并且 copy
mkdir -p ../../rootfs/libc-tests
cp -r ../../prebuilt/linux/libc-test ../../rootfs/libc-tests/libc-test

# 进入 libc-test
cd ../../rootfs/libc-tests/libc-test

# 把默认gcc 替换成 musl-gcc
echo 'CC := musl-gcc' >> config.mak.def

# 编译
echo 正在编译...
make | grep FAIL | wc 
