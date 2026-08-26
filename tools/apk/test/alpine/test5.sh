#!/bin/sh -e

# desc: test post-install script

$APK add --root "$ROOT" --initdb -U --repository "$PWD/repo1" \
	--repository "$SYSREPO" test-d

test -f "$ROOT"/post-install

