#!/bin/sh

TESTDIR=$(realpath "${TESTDIR:-"$(dirname "$0")"/..}")
. "$TESTDIR"/testlib.sh

create_uvol() {
	rm -rf files/uvol/
	mkdir -p files/uvol/
	cat <<EOF > files/uvol/"$1"
$2
EOF
	$APK mkpkg -I name:uvol-"$1" -I version:1.0 -I layer:1 -F files -o uvol-"$1"-1.0.apk

}

reset_uvol_db() {
	rm -rf "$TEST_ROOT/lib/apk/db-uvol"
	mkdir -p "$TEST_ROOT/lib/apk/db-uvol"
	touch "$TEST_ROOT/lib/apk/db-uvol/world"
}

setup_apkroot
create_uvol data "Hello world!"
create_uvol scriptfail "Data for testing failing script!"

APK="$APK --allow-untrusted --no-interactive --force-no-chroot --uvol-manager $TESTDIR/uvol-test-manager.sh"

$APK add --initdb $TEST_USERMODE

reset_uvol_db
$APK add uvol-data-1.0.apk 2>&1 | diff -u /dev/fd/4 4<<EOF - || assert "wrong scripts result"
(1/1) Installing uvol-data (1.0)
uvol(create): uvol-test: create data 13 ro
uvol(write): uvol-test: write data 13
uvol(write): uvol-test: drained input
uvol(up): uvol-test: up data
OK: 13 B in 1 packages
EOF

reset_uvol_db
! $APK add uvol-scriptfail-1.0.apk 2>&1 | diff -u - /dev/fd/4 4<<EOF || assert "wrong scripts result"
(1/1) Installing uvol-scriptfail (1.0)
uvol(create): uvol-test: create scriptfail 33 ro
uvol(write): uvol-test: write scriptfail 33
ERROR: uvol(write): exited with error 2
uvol(remove): uvol-test: remove scriptfail
ERROR: uvol-scriptfail-1.0: failed to extract uvol/scriptfail: uvol error
1 error; 33 B in 1 packages
EOF

exit 0
