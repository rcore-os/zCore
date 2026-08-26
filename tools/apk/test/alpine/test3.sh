#!/bin/sh -e

# desc: test successful pre-install

$APK add --root "$ROOT" --initdb --repository "$PWD/repo1" --repository "$SYSREPO" \
	-U test-c

# check that package was installed
$APK info --root "$ROOT" -e test-c

# check if pre-install was executed
test -f "$ROOT"/pre-install
