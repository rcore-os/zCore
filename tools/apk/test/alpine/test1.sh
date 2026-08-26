#!/bin/sh -e

# desc: test if basic add/del/upgrade works

$APK add --root "$ROOT" --initdb --repository "$PWD/repo1" test-a

test "$("$ROOT"/usr/bin/test-a)" = "hello from test-a-1.0"

$APK upgrade --root "$ROOT" --repository "$PWD/repo2"

test "$("$ROOT"/usr/bin/test-a)" = "hello from test-a-1.1"

$APK del --root "$ROOT" test-a

[ -x "$ROOT/usr/bin/test-a" ] || true
