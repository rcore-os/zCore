#!/bin/sh

TESTDIR=$(realpath "${TESTDIR:-"$(dirname "$0")"/..}")
. "$TESTDIR"/testlib.sh

setup_tmp

mkdir -p etc
uid=$(id -u)
gid=$(id -g)
echo "root:x:${uid}:${gid}:root:/root:/bin/sh" > etc/passwd
echo "root:x:${gid}:root" > etc/group
mkdir -p files/usr/share/foo/bar

$APK --root=. mkpkg --no-xattrs --compat=3.0.0_pre2 -I name:compat -I version:1.0 -F files -o compat-1.0.apk
$APK adbdump compat-1.0.apk | sed -n '/^paths:/,$p' | diff -u /dev/fd/4 4<<EOF - || assert "wrong fetch result"
paths: # 5 items
  - acl:
      mode: 0755
      user: root
      group: root
  - name: usr
    acl:
      mode: 0755
      user: root
      group: root
  - name: usr/share
    acl:
      mode: 0755
      user: root
      group: root
  - name: usr/share/foo
    acl:
      mode: 0755
      user: root
      group: root
  - name: usr/share/foo/bar
    acl:
      mode: 0755
      user: root
      group: root
EOF

$APK --root=. mkpkg --no-xattrs --compat=3.0.0_pre3 -I name:compat -I version:1.0 -F files -o compat-1.0.apk
$APK adbdump compat-1.0.apk | sed -n '/^paths:/,$p' | diff -u /dev/fd/4 4<<EOF - || assert "wrong fetch result"
paths: # 4 items
  - name: usr
    acl:
      mode: 0755
      user: root
      group: root
  - name: usr/share
    acl:
      mode: 0755
      user: root
      group: root
  - name: usr/share/foo
    acl:
      mode: 0755
      user: root
      group: root
  - name: usr/share/foo/bar
    acl:
      mode: 0755
      user: root
      group: root
EOF

$APK --root=. mkpkg --no-xattrs --compat=3.0.0_rc9 -I name:compat -I version:1.0 -F files -o compat-1.0.apk
$APK adbdump compat-1.0.apk | sed -n '/^paths:/,$p' | diff -u /dev/fd/4 4<<EOF - || assert "wrong fetch result"
paths: # 1 items
  - name: usr/share/foo/bar
    acl:
      mode: 0755
      user: root
      group: root
EOF
