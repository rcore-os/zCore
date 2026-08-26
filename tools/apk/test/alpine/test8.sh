#!/bin/sh -e

# desc: test if upgrade works when package is missing in repo

$APK add --root "$ROOT" --initdb --repository "$PWD/repo1" test-a

$APK upgrade --root "$ROOT"
