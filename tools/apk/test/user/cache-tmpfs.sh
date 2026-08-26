#!/bin/sh

TESTDIR=$(realpath "${TESTDIR:-"$(dirname "$0")"/..}")
. "$TESTDIR"/testlib.sh

setup_repo() {
	local repo="$1"
	mkdir -p files/a
	echo hello > files/a/hello

	mkdir -p "$repo"
	$APK mkpkg -I name:hello -I version:1.0 -F files -o "$repo"/hello-1.0.apk
	$APK mkpkg -I name:meta -I version:1.0 -I depends:"hello" -o "$repo"/meta-1.0.apk
	$APK mkndx "$repo"/*.apk -o "$repo"/index.adb
}

APK="$APK --allow-untrusted --no-interactive"
setup_apkroot
setup_repo "$PWD/repo"

mkdir -p "$TEST_ROOT"/etc/apk/cache
$APK add --initdb $TEST_USERMODE --repository "test:/$PWD/repo/index.adb" meta

# reinstall from cache
$APK del meta
$APK add --initdb $TEST_USERMODE --no-network --repository "test:/$PWD/repo/index.adb" meta

# make sure fetch still works
$APK fetch --repository "test:/$PWD/repo/index.adb" meta
[ -f meta-1.0.apk ] || assert "meta package not fetched"
