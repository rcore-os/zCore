#!/bin/sh

TESTDIR=$(realpath "${TESTDIR:-"$(dirname "$0")"/..}")
. "$TESTDIR"/testlib.sh

case "$($APK version --help 2>/dev/null)" in
apk-tools*', compiled for '*.*) ;;
*) assert "expected help" ;;
esac
case "$($APK --unknown-option version 2>&1 >/dev/null)" in
*'unrecognized option'*'unknown-option'*) ;;
*) assert "expected unknown option error" ;;
esac
case "$($APK mkpkg --compression AAA 2>&1 >/dev/null)" in
*'invalid argument'*'compression'*'AAA'*) ;;
*) assert "expeected invalid argument error" ;;
esac
case "$($APK --force- 2>&1 >/dev/null)" in
*"ambiguous option 'force-'"*) ;;
*) assert "expected ambiguous error" ;;
esac
case "$($APK --no- 2>&1 >/dev/null)" in
*"ambiguous option 'no-'"*) ;;
*) assert "expected ambiguous error" ;;
esac
case "$($APK --no-cache 2>&1 >/dev/null)" in
"") ;;
*) assert "expected valid exact option" ;;
esac
case "$($APK --no-cache=foo 2>&1 >/dev/null)" in
*"option 'no-cache' does not expect argument"*) ;;
*) assert "expected no argument error" ;;
esac
case "$($APK --cache=no 2>&1 >/dev/null)" in
"") ;;
*) assert "expected no argument error" ;;
esac
case "$($APK --root 2>&1 >/dev/null)" in
*"option 'root' expects an argument"*) ;;
*) assert "expected argument error" ;;
esac
case "$($APK -v  -- -proot non-existent 2>&1 >/dev/null)" in
*"'-proot' is not an apk command"*) ;;
*) assert "expected argument error" ;;
esac
