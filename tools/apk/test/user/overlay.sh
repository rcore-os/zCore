#!/bin/sh

TESTDIR=$(realpath "${TESTDIR:-"$(dirname "$0")"/..}")
. "$TESTDIR"/testlib.sh

setup_apkroot
APK="$APK --allow-untrusted --no-interactive --force-no-chroot"

mkdir -p pkg/etc pkg/data "$TEST_ROOT"/etc "$TEST_ROOT"/data
for f in etc/a etc/b etc/c data/d data/e; do
	echo "package" > pkg/"$f"
	echo "overlay" > "$TEST_ROOT"/"$f"
done

$APK mkpkg -F pkg -I name:overlay -I version:1.0 -o overlay-1.0.apk

$APK add --initdb $TEST_USERMODE --overlay-from-stdin overlay-1.0.apk > apk-stdout.log 2>&1 <<EOF || assert "install fail"
etc/b
data/e
EOF

diff -u - apk-stdout.log <<EOF || assert "wrong scripts result"
(1/1) Installing overlay (1.0)
  Installing file to etc/a.apk-new
  Installing file to etc/c.apk-new
OK: 40 B in 1 packages
EOF

cd "$TEST_ROOT"
[ "$(cat etc/a)" = "overlay" ] || assert "etc/a updated unexpectedly"
[ "$(cat etc/a.apk-new)" = "package" ] || assert "etc/a.apk-new missing"
[ "$(cat etc/b)" = "overlay" ] || assert "etc/b updated unexpectedly"
[ ! -e "etc/b.apk-new" ] || assert "etc/b.apk-new exists"
[ "$(cat etc/c)" = "overlay" ] || assert "etc/c updated unexpectedly"
[ "$(cat etc/c.apk-new)" = "package" ] || assert "etc/c.apk-new missing"
[ "$(cat data/d)" = "package" ] || assert "data/d updated unexpectedly"
[ "$(cat data/e)" = "overlay" ] || assert "data/e updated unexpectedly"
