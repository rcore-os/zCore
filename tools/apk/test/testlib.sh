#!/bin/sh

# shellcheck disable=SC2034 # various variables are not used always

set -eo pipefail

assert() {
	echo "$*"
	exit 1
}

glob_one() {
	# shellcheck disable=SC2048 # argument is wildcard needing expansion
	for a in $*; do
		if [ -e "$a" ]; then
			echo "$a"
			return 0
		fi
	done
	return 1
}

setup_tmp() {
	TMPDIR=$(mktemp -d -p /tmp apktest.XXXXXXXX)
	[ -d "$TMPDIR" ] || return 1
	# shellcheck disable=SC2064 # expand TMPDIR here
	trap "rm -rf -- '$TMPDIR'" EXIT
	cd "$TMPDIR"
}

setup_apkroot() {
	TEST_USERMODE=""
	[ "$(id -u)" = 0 ] || TEST_USERMODE="--usermode"

	TEST_ROOT=$(mktemp -d -p /tmp apktest.XXXXXXXX)
	[ -d "$TEST_ROOT" ] || return 1

	# shellcheck disable=SC2064 # expand TMPDIR here
	trap "rm -rf -- '$TEST_ROOT'" EXIT
	APK="$APK --root $TEST_ROOT"

	mkdir -p "$TEST_ROOT/etc/apk/cache" \
		"$TEST_ROOT/lib/apk/db" \
		"$TEST_ROOT/tmp" \
		"$TEST_ROOT/var/log"

	touch "$TEST_ROOT/etc/apk/world"
	touch "$TEST_ROOT/lib/apk/db/installed"
	ln -sf /dev/null "$TEST_ROOT/var/log/apk.log"
	cd "$TEST_ROOT/tmp"
}

[ "$APK" ] || assert "APK environment variable not set"
