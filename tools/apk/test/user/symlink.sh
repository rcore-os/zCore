#!/bin/sh

TESTDIR=$(realpath "${TESTDIR:-"$(dirname "$0")"/..}")
. "$TESTDIR"/testlib.sh

setup_apkroot
APK="$APK --allow-untrusted --no-interactive"

mkdir -p files/data
echo hello > files/data/hello.txt
ln -s hello.txt files/data/hello.link
ln -s nonexistent.txt files/data/broken.link

$APK mkpkg -I name:symlink -I version:1.0 -F files -o symlink-1.0.apk
$APK add --initdb $TEST_USERMODE symlink-1.0.apk

[ "$(readlink "$TEST_ROOT"/data/hello.link)" = "hello.txt" ] || assert "hello.link"
[ "$(readlink "$TEST_ROOT"/data/broken.link)" = "nonexistent.txt" ] || assert "broken.link"
