#!/bin/sh -e

# desc: test failing pre-install

# pre-install script will fail if should-fail file exists
mkdir -p "$ROOT"
touch "$ROOT"/should-fail

$APK add --root "$ROOT" --initdb --repository "$PWD/repo1" --repository "$SYSREPO" \
	-U test-c && exit 1

# check that pre-install was executed
test -f "$ROOT"/pre-install

# check that package was installed
$APK info --root "$ROOT" -e test-c

