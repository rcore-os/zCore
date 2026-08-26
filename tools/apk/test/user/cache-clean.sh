#!/bin/sh

TESTDIR=$(realpath "${TESTDIR:-"$(dirname "$0")"/..}")
. "$TESTDIR"/testlib.sh

setup_apkroot
APK="$APK --allow-untrusted --no-interactive"

mkdir a b
touch a/a b/b

$APK mkpkg -I name:test-a -I version:1.0 -F a -o test-a-1.0.apk
$APK mkpkg -I name:test-b -I version:1.0 -F b -o test-b-1.0.apk
$APK add --initdb $TEST_USERMODE  test-a-1.0.apk test-b-1.0.apk

CACHED_A=$(glob_one "$TEST_ROOT/etc/apk/cache/test-a-1.0.*.apk")
CACHED_B=$(glob_one "$TEST_ROOT/etc/apk/cache/test-b-1.0.*.apk")

CACHED_B2="$TEST_ROOT/etc/apk/cache/test-b-1.0.xeeb78f1.apk"
CACHED_C=$(echo "$CACHED_B" | sed 's,test-b,test-c,')

[ -f "$CACHED_A" ] || assert "cached test-a not preset"
[ -f "$CACHED_B" ] || assert "cached test-b not preset"
[ -f "$CACHED_B2" ] && assert "cached test-b not preset"
[ -f "$CACHED_C" ] && assert "cached test-c preset"

touch "$CACHED_C" "$CACHED_B2"
dd if=/dev/zero of="$CACHED_B" bs=1024 count=1 > /dev/null 2>&1

$APK cache clean -vv

[ -f "$CACHED_A" ] || assert "cached test-a deleted"
[ -f "$CACHED_B" ] && assert "cached test-b not deleted"
[ -f "$CACHED_B2" ] && assert "cached test-b not deleted"
[ -f "$CACHED_C" ] && assert "cached test-c not deleted"
exit 0
