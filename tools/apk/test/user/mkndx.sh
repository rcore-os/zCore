#!/bin/sh

# shellcheck disable=SC2016 # no expansion for pkgname-spec

TESTDIR=$(realpath "${TESTDIR:-"$(dirname "$0")"/..}")
. "$TESTDIR"/testlib.sh

setup_apkroot
APK="$APK --allow-untrusted --no-interactive --no-cache"

$APK mkpkg && assert "no parameters is an error"
[ $? = 99 ] || assert "wrong error code"

$APK mkpkg -I name:aaa -I version:1.0 -o aaa-1.0.apk
$APK mkpkg -I name:test-a -I version:1.0 -I tags:"tagA tagC=1" -o test-a-1.0.apk
$APK mkpkg -I name:test-a -I version:2.0 -o test-a-2.0.apk
$APK mkpkg -I name:test-a -I version:3.0 -o test-a-3.0.apk
$APK mkpkg -I name:test-b -I version:1.0 -I tags:"tagB tagC=2" -o test-b-1.0.apk
$APK mkpkg -I name:test-c -I version:1.0 -I "recommends:test-a" -o test-c-1.0.apk

$APK mkpkg -I name:bad-a -I version:1.0 -I tags:"lost&found" -o bad-a-1.0.apk 2>/dev/null && assert "invalid tag allowed"
[ -e bad-a-1.0.apk ] && assert "bad-a should not exist"

$APK mkndx -q -o index.adb test-a-1.0.apk
$APK mkndx -vv -o index-reindex.adb -x index.adb test-a-1.0.apk test-b-1.0.apk | diff -u /dev/fd/4 4<<EOF - || assert "wrong mkndx result"
test-a-1.0.apk: indexed from old index
test-b-1.0.apk: indexed new package
Index has 2 packages (of which 1 are new)
EOF

$APK mkndx --pkgname-spec 'https://test/${name}-${version}.apk' -o index.adb test-a-1.0.apk test-b-1.0.apk
$APK fetch --url --simulate --from none --repository index.adb --pkgname-spec '${name}_${version}.pkg' test-a test-b 2>&1 | diff -u /dev/fd/4 4<<EOF - || assert "wrong fetch result"
https://test/test-a-1.0.apk
https://test/test-b-1.0.apk
EOF

$APK mkndx --pkgname-spec '${name:3}/${name}-${version}.apk' -o index.adb test-a-1.0.apk test-b-1.0.apk
$APK fetch --url --simulate --from none --repository "test:/$PWD/index.adb" --pkgname-spec '${name}_${version}.pkg' test-a test-b 2>&1 | diff -u /dev/fd/4 4<<EOF - || assert "wrong fetch result"
test:/$PWD/tes/test-a-1.0.apk
test:/$PWD/tes/test-b-1.0.apk
EOF

$APK mkndx --pkgname-spec '${name:3}/${name}-${version}.apk' -o index.adb test-a-1.0.apk test-b-1.0.apk test-c-1.0.apk
$APK fetch --url --simulate --from none --repository index.adb --pkgname-spec '${name}_${version}.pkg' test-a test-b 2>&1 | diff -u /dev/fd/4 4<<EOF - || assert "wrong fetch result"
./tes/test-a-1.0.apk
./tes/test-b-1.0.apk
EOF

$APK mkndx -vv -o index-unfiltered.adb aaa-1.0.apk test-a-1.0.apk test-a-2.0.apk test-a-3.0.apk test-b-1.0.apk test-c-1.0.apk
$APK mkndx -vv --filter-spec '${name}-${version}' --pkgname-spec 'http://test/${name}-${version}.apk' -x index-unfiltered.adb -o index-filtered.adb test-a-1.0 aaa-1.0 test-c-1.0
$APK fetch --url --simulate --from none --repository index-filtered.adb --pkgname-spec '${name}_${version}.pkg' "*" 2>&1 | diff -u /dev/fd/4 4<<EOF - || assert "wrong fetch result"
http://test/aaa-1.0.apk
http://test/test-a-1.0.apk
http://test/test-c-1.0.apk
EOF

$APK query --format=yaml --repository index.adb --fields name,recommends "test-c" 2>&1 | diff -u /dev/fd/4 4<<EOF - || assert "wrong fetch result"
# 1 items
- name: test-c
  recommends: # 1 items
    - test-a
EOF

$APK query --format yaml --repository index.adb --fields name,tags --match tags tagA 2>&1 | diff -u /dev/fd/4 4<<EOF - || assert "wrong query tags result"
# 1 items
- name: test-a
  tags: # 2 items
    - tagA
    - tagC=1
EOF

$APK query --format yaml --repository index.adb --fields name,tags --match tags "tagC=*" 2>&1 | diff -u /dev/fd/4 4<<EOF - || assert "wrong query tags result"
# 2 items
- name: test-a
  tags: # 2 items
    - tagA
    - tagC=1
- name: test-b
  tags: # 2 items
    - tagB
    - tagC=2
EOF
