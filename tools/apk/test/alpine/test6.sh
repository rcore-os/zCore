#!/bin/sh -e

# desc: test triggers in kernel package

$APK add --root "$ROOT" --initdb -U --repository "$PWD/repo1" \
	--repository "$SYSREPO" alpine-keys alpine-baselayout linux-lts linux-firmware-none

test -e "$ROOT"/boot/vmlinuz-lts

test -e "$ROOT"/boot/initramfs-lts

