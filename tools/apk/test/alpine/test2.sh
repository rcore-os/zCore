#!/bin/sh -e

# desc: test if dependencies works

# test-b depends on test-a
$APK add --root "$ROOT" --initdb --repository "$PWD/repo1" test-b

# check if test-a was installed
test "$("$ROOT"/usr/bin/test-a)" = "hello from test-a-1.0"

# run an upgrade
$APK upgrade --root "$ROOT" --repository "$PWD/repo2"

# test if test-a was upgraded
test "$("$ROOT"/usr/bin/test-a)" = "hello from test-a-1.1"

# remove test-b
$APK del --root "$ROOT" test-b

# test if the dependency was removed too
if [ -x "$ROOT/usr/bin/test-a" ]; then
	exit 1
fi
