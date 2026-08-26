#!/bin/sh

TESTDIR=$(realpath "${TESTDIR:-"$(dirname "$0")"/..}")
. "$TESTDIR"/testlib.sh

if ! stat -c "%D:%i" /dev/null > /dev/null 2>&1; then
	dev_inode() {
		stat -f "%Xd:%i" "$@"
	}
else
	dev_inode() {
		stat -c "%D:%i" "$@"
	}
fi

setup_apkroot
APK="$APK --allow-untrusted --no-interactive"

mkdir -p files/a files/b
echo hello > files/a/zzz
ln files/a/zzz files/a/aaa
ln files/a/zzz files/a/bbb

echo hello > files/b/zzz
ln files/b/zzz files/b/aaa
ln files/b/zzz files/b/bbb

$APK mkpkg -I name:hardlink -I version:1.0 -F files -o hardlink-1.0.apk
$APK add --initdb $TEST_USERMODE hardlink-1.0.apk

cd "$TEST_ROOT"
A_INODE="$(dev_inode a/aaa)"
B_INODE="$(dev_inode b/aaa)"
[ "$A_INODE" != "$B_INODE" ] || assert "a != b"
[ "$(dev_inode a/bbb)" = "$A_INODE" ] || assert "a/bbb"
[ "$(dev_inode a/zzz)" = "$A_INODE" ] || assert "a/zzz"
[ "$(dev_inode b/bbb)" = "$B_INODE" ] || assert "b/bbb"
[ "$(dev_inode b/zzz)" = "$B_INODE" ] || assert "b/zzz"
