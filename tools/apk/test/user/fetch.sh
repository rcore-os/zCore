#!/bin/sh

TESTDIR=$(realpath "${TESTDIR:-"$(dirname "$0")"/..}")
. "$TESTDIR"/testlib.sh

setup_repo() {
	local repo="$1"
	mkdir -p files/a
	echo hello > files/a/hello

	mkdir -p "$repo"
	$APK mkpkg -I name:hello -I arch:noarch -I version:1.0 -F files -o "$repo"/hello-1.0.apk
	$APK mkpkg -I name:strange -I arch:strange -I version:1.0 -F files -o "$repo"/strange-1.0.apk
	$APK mkpkg -I name:meta -I arch:noarch -I version:1.0 -I depends:"hello" -o "$repo"/meta-1.0.apk
	$APK mkndx "$repo"/*.apk -o "$repo"/index.adb
}

assert_downloaded() {
	for f in "$@"; do
		[ -f "$f" ] || assert "failed to fetch $f"
		rm "$f"
	done
	for f in *.*; do
		[ -f "$f" ] && assert "fetched extra file $f"
	done
	return 0
}

APK="$APK --allow-untrusted --no-interactive"
setup_tmp
setup_repo "$PWD/repo"

APK="$APK --from none --repository test:/$PWD/repo/index.adb --no-cache"
$APK fetch meta
assert_downloaded meta-1.0.apk

$APK fetch --recursive meta
assert_downloaded meta-1.0.apk hello-1.0.apk

# shellcheck disable=SC2016 # no expansion for pkgname-spec
$APK fetch --pkgname-spec '${name}_${version}_${arch}.pkg' --recursive meta
assert_downloaded meta_1.0_noarch.pkg hello_1.0_noarch.pkg

$APK fetch --arch strange --recursive strange
assert_downloaded strange-1.0.apk
