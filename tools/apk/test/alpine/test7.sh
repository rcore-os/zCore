#!/bin/sh -e

# desc: test triggers in busybox package

# we had a bug that caused apk fix --reinstall to segfault every second time

$APK add --root "$ROOT" --initdb -U --repository "$PWD/repo1" \
	--repository "$SYSREPO" busybox

# shellcheck disable=SC2034 # i is unused
for i in 0 1 2 3; do
	# delete wget symlink
	rm -f "$ROOT"/usr/bin/wget

	# re-install so we run the trigger again
	$APK fix --root "$ROOT" --repository "$SYSREPO" --reinstall  busybox

	# verify wget symlink is there
	test -L "$ROOT"/usr/bin/wget
done


