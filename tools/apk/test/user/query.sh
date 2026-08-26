#!/bin/sh

TESTDIR=$(realpath "${TESTDIR:-"$(dirname "$0")"/..}")
. "$TESTDIR"/testlib.sh

installed_db="$(realpath "$(dirname "$0")/query-installed.data")"
setup_apkroot
cp "$installed_db" "$TEST_ROOT"/lib/apk/db/installed

APK="$APK --no-network"

$APK info apk-tools 2>&1 | diff -u /dev/fd/4 4<<EOF - || assert "wrong result"
apk-tools-2.14.6-r3 description:
Alpine Package Keeper - package manager for alpine

apk-tools-2.14.6-r3 webpage:
https://gitlab.alpinelinux.org/alpine/apk-tools

apk-tools-2.14.6-r3 installed size:
247 KiB

EOF

! $APK info -W sbin/apk usr/lib/libapk.so.2.14.0 bin/not-found usr 2>&1 | diff -u /dev/fd/4 4<<EOF - || assert "wrong result"
sbin/apk is owned by apk-tools-2.14.6-r3
ERROR: bin/not-found: Could not find owner package
usr/lib/libapk.so.2.14.0 is owned by apk-tools-2.14.6-r3
usr is owned by alpine-baselayout-3.6.8-r1
EOF

$APK info -qW sbin/apk usr/lib/libapk.so.2.14.0 usr 2>&1 | diff -u /dev/fd/4 4<<EOF - || assert "wrong result"
alpine-baselayout
apk-tools
EOF

$APK info --all scanelf 2>&1 | diff -u /dev/fd/4 4<<EOF - || assert "wrong result"
scanelf-1.3.8-r1 description:
Scan ELF binaries for stuff

scanelf-1.3.8-r1 webpage:
https://wiki.gentoo.org/wiki/Hardened/PaX_Utilities

scanelf-1.3.8-r1 installed size:
65 KiB

scanelf-1.3.8-r1 depends on:
so:libc.musl-x86_64.so.1

scanelf-1.3.8-r1 provides:
cmd:scanelf=1.3.8-r1

scanelf-1.3.8-r1 is required by:
musl-utils-1.2.5-r9

scanelf-1.3.8-r1 contains:
usr/bin/scanelf

scanelf-1.3.8-r1 triggers:

scanelf-1.3.8-r1 has auto-install rule:

scanelf-1.3.8-r1 affects auto-installation of:

scanelf-1.3.8-r1 replaces:
pax-utils

scanelf-1.3.8-r1 license:
GPL-2.0-only

EOF

$APK list --installed 2>&1 | diff -u /dev/fd/4 4<<EOF - || assert "wrong result"
alpine-base-3.21.3-r0 x86_64 {alpine-base} (MIT) [installed]
alpine-baselayout-3.6.8-r1 x86_64 {alpine-baselayout} (GPL-2.0-only) [installed]
alpine-baselayout-data-3.6.8-r1 x86_64 {alpine-baselayout} (GPL-2.0-only) [installed]
alpine-conf-3.19.2-r0 x86_64 {alpine-conf} (MIT) [installed]
alpine-keys-2.5-r0 x86_64 {alpine-keys} (MIT) [installed]
alpine-release-3.21.3-r0 x86_64 {alpine-base} (MIT) [installed]
apk-tools-2.14.6-r3 x86_64 {apk-tools} (GPL-2.0-only) [installed]
apk-tools-doc-2.14.6-r3 x86_64 {apk-tools} (GPL-2.0-only) [installed]
bash-5.2.37-r0 x86_64 {bash} (GPL-3.0-or-later) [installed]
bash-doc-5.2.37-r0 x86_64 {bash} (GPL-3.0-or-later) [installed]
busybox-1.37.0-r12 x86_64 {busybox} (GPL-2.0-only) [installed]
busybox-binsh-1.37.0-r12 x86_64 {busybox} (GPL-2.0-only) [installed]
busybox-doc-1.37.0-r12 x86_64 {busybox} (GPL-2.0-only) [installed]
busybox-mdev-openrc-1.37.0-r12 x86_64 {busybox} (GPL-2.0-only) [installed]
busybox-openrc-1.37.0-r12 x86_64 {busybox} (GPL-2.0-only) [installed]
busybox-suid-1.37.0-r12 x86_64 {busybox} (GPL-2.0-only) [installed]
ca-certificates-bundle-20241121-r1 x86_64 {ca-certificates} (MPL-2.0 AND MIT) [installed]
docs-0.2-r6 x86_64 {docs} (MIT) [installed]
ifupdown-ng-0.12.1-r6 x86_64 {ifupdown-ng} (ISC) [installed]
ifupdown-ng-doc-0.12.1-r6 x86_64 {ifupdown-ng} (ISC) [installed]
libcap2-2.71-r0 x86_64 {libcap} (BSD-3-Clause OR GPL-2.0-only) [installed]
libcrypto3-3.3.3-r0 x86_64 {openssl} (Apache-2.0) [installed]
libncursesw-6.5_p20241006-r3 x86_64 {ncurses} (X11) [installed]
libssl3-3.3.3-r0 x86_64 {openssl} (Apache-2.0) [installed]
man-pages-6.9.1-r0 x86_64 {man-pages} (GPL-2.0-or-later) [installed]
mandoc-1.14.6-r13 x86_64 {mandoc} (ISC) [installed]
mandoc-doc-1.14.6-r13 x86_64 {mandoc} (ISC) [installed]
mdev-conf-4.7-r0 x86_64 {mdev-conf} (MIT) [installed]
musl-1.2.5-r9 x86_64 {musl} (MIT) [installed]
musl-utils-1.2.5-r9 x86_64 {musl} (MIT AND BSD-2-Clause AND GPL-2.0-or-later) [installed]
ncurses-terminfo-base-6.5_p20241006-r3 x86_64 {ncurses} (X11) [installed]
openrc-0.55.1-r2 x86_64 {openrc} (BSD-2-Clause) [installed]
openrc-doc-0.55.1-r2 x86_64 {openrc} (BSD-2-Clause) [installed]
readline-8.2.13-r0 x86_64 {readline} (GPL-3.0-or-later) [installed]
readline-doc-8.2.13-r0 x86_64 {readline} (GPL-3.0-or-later) [installed]
scanelf-1.3.8-r1 x86_64 {pax-utils} (GPL-2.0-only) [installed]
ssl_client-1.37.0-r12 x86_64 {busybox} (GPL-2.0-only) [installed]
zlib-1.3.1-r2 x86_64 {zlib} (Zlib) [installed]
zlib-doc-1.3.1-r2 x86_64 {zlib} (Zlib) [installed]
EOF

$APK list --installed --origin apk-tools 2>&1 | diff -u /dev/fd/4 4<<EOF - || assert "wrong result"
apk-tools-2.14.6-r3 x86_64 {apk-tools} (GPL-2.0-only) [installed]
apk-tools-doc-2.14.6-r3 x86_64 {apk-tools} (GPL-2.0-only) [installed]
EOF

$APK query --format yaml --installed --fields all "apk-tools" 2>&1 | diff -u /dev/fd/4 4<<EOF - || assert "wrong result"
# 1 items
- package: apk-tools-2.14.6-r3
  name: apk-tools
  version: 2.14.6-r3
  description: Alpine Package Keeper - package manager for alpine
  arch: x86_64
  license: GPL-2.0-only
  origin: apk-tools
  maintainer: Natanael Copa <ncopa@alpinelinux.org>
  url: https://gitlab.alpinelinux.org/alpine/apk-tools
  commit: 41847d6ccff08940b5bf1ba0d6005e95897039f9
  build-time: 1739483850
  installed-size: 253640
  file-size: 122059
  depends: # 6 items
    - musl>=1.2.3_git20230424
    - ca-certificates-bundle
    - so:libc.musl-x86_64.so.1
    - so:libcrypto.so.3
    - so:libssl.so.3
    - so:libz.so.1
  provides: # 2 items
    - so:libapk.so.2.14.0=2.14.0
    - cmd:apk=2.14.6-r3
  repositories:
    - lib/apk/db/installed
  reverse-depends:
    - alpine-base
  reverse-install-if:
    - apk-tools-doc
  contents:
    - sbin/apk
    - usr/lib/libapk.so.2.14.0
  status:
    - installed
EOF

$APK query --format yaml --installed --fields package,reverse-depends,reverse-install-if:package "apk-tools" 2>&1 | diff -u /dev/fd/4 4<<EOF - || assert "wrong result"
# 1 items
- package: apk-tools-2.14.6-r3
  reverse-depends:
    - alpine-base-3.21.3-r0
  reverse-install-if:
    - apk-tools-doc-2.14.6-r3
EOF

$APK query --format yaml --installed --fields package,reverse-depends,reverse-install-if:origin "apk-tools" 2>&1 | diff -u /dev/fd/4 4<<EOF - || assert "wrong result"
# 1 items
- package: apk-tools-2.14.6-r3
  reverse-depends:
    - alpine-base
  reverse-install-if:
    - apk-tools
EOF

$APK query --summarize reverse-install-if:origin "apk*" 2>&1 | diff -u /dev/fd/4 4<<EOF - || assert "wrong result"
apk-tools
EOF

$APK query --format yaml --fields origin,package --match depends "musl>=1.2.3_git20230424" 2>&1 | diff -u /dev/fd/4 4<<EOF - || assert "wrong result"
# 1 items
- package: apk-tools-2.14.6-r3
  origin: apk-tools
EOF

$APK query --format json --installed "musl*" 2>&1 | diff -u /dev/fd/4 4<<EOF - || assert "wrong result"
[
  {
    "name": "musl",
    "version": "1.2.5-r9",
    "description": "the musl c library (libc) implementation",
    "arch": "x86_64",
    "license": "MIT",
    "origin": "musl",
    "url": "https://musl.libc.org/",
    "file-size": 411323
  }, {
    "name": "musl-utils",
    "version": "1.2.5-r9",
    "description": "the musl c library (libc) implementation",
    "arch": "x86_64",
    "license": "MIT AND BSD-2-Clause AND GPL-2.0-or-later",
    "origin": "musl",
    "url": "https://musl.libc.org/",
    "file-size": 36055
  }
]
EOF

$APK search --installed alpine 2>&1 | diff -u /dev/fd/4 4<<EOF - || assert "wrong result"
alpine-base-3.21.3-r0
alpine-baselayout-3.6.8-r1
alpine-baselayout-data-3.6.8-r1
alpine-conf-3.19.2-r0
alpine-keys-2.5-r0
alpine-release-3.21.3-r0
EOF

