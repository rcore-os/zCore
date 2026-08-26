#!/bin/sh

SHELL="${1:-bash}"
SHELLCHECK="${SHELLCHECK:-shellcheck}"
TESTDIR="${TESTDIR:-.}"

cd "$TESTDIR" || exit 1

# SC2001 "See if you can use ${variable//search/replace} instead" on bash conflicts with dash
$SHELLCHECK -x -e SC2001 -s "$SHELL" -- *.sh */*.sh
