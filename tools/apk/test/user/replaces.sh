#!/bin/sh

TESTDIR=$(realpath "${TESTDIR:-"$(dirname "$0")"/..}")
. "$TESTDIR"/testlib.sh

create_pkg() {
	local pkg="$1" ver="$2"
	local pkgdir="files/"${pkg}-${ver}""
	shift 2

	mkdir -p "$pkgdir"/files
	echo "$pkg" > "$pkgdir"/files/test-file

	$APK mkpkg -I "name:${pkg}" -I "version:${ver}" "$@" -F "$pkgdir" -o "${pkg}-${ver}.apk"
}

check_content() {
	local val
	val=$(cat "$TEST_ROOT"/files/test-file) || assert "test-file not found"
	[ "$val" = "$1" ] || assert "file content wrong: $1 expected, got $val"
}

setup_apkroot
APK="$APK --allow-untrusted --no-interactive"

create_pkg a 1.0  -I "tags:tagA tagB"
create_pkg a 2.0  -I "tags:tagA tagB"
create_pkg b 1.0
create_pkg c 1.0 -I "replaces:a"

create_pkg d-a 1.0 -I "origin:d"
create_pkg d-b 1.0 -I "origin:d"

$APK add --initdb $TEST_USERMODE a-1.0.apk
check_content "a"
$APK query --format yaml --fields name,tags,repositories a  | diff -u /dev/fd/4 4<<EOF - || assert "wrong scripts result"
# 1 items
- name: a
  tags: # 2 items
    - tagA
    - tagB
  repositories:
    - lib/apk/db/installed
EOF

$APK add b-1.0.apk && assert "should error with conflicting file"
check_content "a"
$APK del b
$APK add c-1.0.apk || assert "should succeed with replaces"
check_content "c"
$APK add a-2.0.apk || assert "a upgrade should succeed"
check_content "c"
$APK del a c

$APK add d-a-1.0.apk || assert "d-a should succeed"
check_content "d-a"
$APK add d-b-1.0.apk || assert "d-b should succeed due to origin"
check_content "d-b"
