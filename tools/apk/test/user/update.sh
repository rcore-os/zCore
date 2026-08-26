#!/bin/sh

TESTDIR=$(realpath "${TESTDIR:-"$(dirname "$0")"/..}")
. "$TESTDIR"/testlib.sh

setup_repo() {
	local repo="$1"

	mkdir -p "$repo"
	$APK mkpkg -I name:hello -I arch:noarch -I version:1.0 -o "$repo"/hello-1.0.apk
	$APK mkndx -d "test repo" "$repo"/*.apk -o "$repo"/index.adb
}

APK="$APK --allow-untrusted --no-interactive"

setup_apkroot
setup_repo "$PWD/repo"
APK="$APK --repository test:/$PWD/repo/index.adb"

[ "$($APK update 2>&1)" = "test repo [test:/$PWD/repo/index.adb]
OK: 1 distinct packages available" ] || assert "update fail"
INDEX=$(glob_one "$TEST_ROOT/etc/apk/cache/APKINDEX.*.tar.gz") || assert "update fail"
touch -r "$INDEX" orig-stamp
sleep 1

[ "$($APK update --cache-max-age 10 2>&1)" = "test repo [test:/$PWD/repo/index.adb]
OK: 1 distinct packages available" ] || assert "update fail"
[ "$INDEX" -nt orig-stamp ] && assert "caching failed"

[ "$($APK update --update-cache  2>&1)" = "test repo [test:/$PWD/repo/index.adb]
OK: 1 distinct packages available" ] || assert "update fail"
[ "$INDEX" -nt orig-stamp ] || assert "refresh fail"

[ "$($APK update --no-cache 2>&1)" = "test repo [test:/$PWD/repo/index.adb]
OK: 1 distinct packages available" ] || assert "update --no-cache fail"
