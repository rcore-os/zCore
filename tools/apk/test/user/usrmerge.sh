#!/bin/sh

TESTDIR=$(realpath "${TESTDIR:-"$(dirname "$0")"/..}")
. "$TESTDIR"/testlib.sh

create_pkg() {
	local ver="$1" prefix="$2"
	local pkgdir="files/"a-${ver}""

	mkdir -p "$pkgdir"/etc
	echo "package $ver" > "$pkgdir"/etc/file
	echo "package $ver" > "$pkgdir/etc/file-$ver"

	mkdir -p "$pkgdir"/usr/lib
	echo "package $ver" > "$pkgdir"/usr/lib/data
	echo "package $ver" > "$pkgdir/usr/lib/data-$ver"

	mkdir -p "$pkgdir/$prefix"/lib
	echo "package $ver" > "$pkgdir/$prefix"/lib/file
	echo "package $ver" > "$pkgdir/$prefix/lib/file-$ver"

	$APK mkpkg -I name:test-a -I "version:${ver}" -F "$pkgdir" -o "test-a-${ver}.apk"
}

setup_apkroot
APK="$APK --allow-untrusted --no-interactive"

create_pkg 1.0 ""
create_pkg 2.0 "/usr"

$APK add --initdb $TEST_USERMODE test-a-1.0.apk
cd "$TEST_ROOT"
[ -e etc/file ] || assert "etc file not found"
[ -e etc/file-1.0 ] || assert "etc file not found"
[ -e usr/lib/data-1.0 ] || assert "usr/lib file not found"
[ -e usr/lib/data-1.0 ] || assert "usr/lib file not found"
[ -e lib/file ] || assert "lib file not found"
[ -e lib/file-1.0 ] || assert "lib file not found"
cd - > /dev/null

# manual usr-merge
mv "$TEST_ROOT"/lib/* "$TEST_ROOT"/usr/lib
rmdir "$TEST_ROOT"/lib
ln -s usr/lib "$TEST_ROOT"/lib

$APK add -vv test-a-2.0.apk
cd "$TEST_ROOT"
[ -e etc/file ] || assert "etc file not found"
[ -e etc/file-1.0 ] && assert "etc file not removed"
[ -e etc/file-2.0 ] || assert "etc file not found"
[ -e usr/lib/data ] || assert "usr/lib file not found"
[ -e usr/lib/data-1.0 ] && assert "usr/lib file not removed"
[ -e usr/lib/data-2.0 ] || assert "usr/lib file not found"
[ -e usr/lib/file ] || assert "moved lib file not found"
[ -e usr/lib/file-1.0 ] && assert "moved lib file not removed"
[ -e usr/lib/file-2.0 ] || assert "moved lib file not found"
cd - > /dev/null

exit 0
