#!/bin/sh

TESTDIR=$(realpath "${TESTDIR:-"$(dirname "$0")"}")
. "$TESTDIR"/testlib.sh

update_repo() {
	local repo="$1"
	if [ ! -f "$repo.tar.gz" ] || [ "$repo" -nt "$repo.tar.gz" ]; then
		local tmpname="$repo.tar.gz.$$"
		ln -snf "$repo" APKINDEX
		tar chzf "$tmpname" APKINDEX
		rm APKINDEX
		mv "$tmpname" "$repo.tar.gz"
	fi
}

run_test() {
	local test="$1"
	local testfile testdir

	testfile="$(realpath "$test")"
	testdir="$(dirname "$testfile")"

	setup_apkroot
	mkdir -p "$TEST_ROOT/data/src"

	local args="" repo run_found
	exec 4> /dev/null
	while IFS="" read -r ln; do
		case "$ln" in
		"@ARGS "*)
			args="$args ${ln#* }"
			run_found=yes
			;;
		"@WORLD "*)
			for dep in ${ln#* }; do
				echo "$dep"
			done > "$TEST_ROOT/etc/apk/world"
			;;
		"@INSTALLED "*)
			ln -snf "$testdir/${ln#* }" "$TEST_ROOT/lib/apk/db/installed"
			;;
		"@REPO @"*)
			tag="${ln#* }"
			repo="${tag#* }"
			tag="${tag% *}"
			update_repo "$testdir/$repo"
			echo "$tag test:/$testdir/$repo.tar.gz" >> "$TEST_ROOT"/etc/apk/repositories
			;;
		"@REPO "*)
			repo="${ln#* }"
			update_repo "$testdir/$repo"
			echo "test:/$testdir/$repo.tar.gz" >> "$TEST_ROOT"/etc/apk/repositories
			;;
		"@CACHE "*)
			ln -snf "$testdir/${ln#* }" "$TEST_ROOT/etc/apk/cache/installed"
			;;
		"@EXPECT")
			exec 4> "$TEST_ROOT/data/expected"
			;;
		"@"*)
			echo "$test: invalid spec: $ln"
			run_found=""
			break
			;;
		*)
			echo "$ln" >&4
			;;
		esac
	done < "$testfile"
	exec 4> /dev/null

	[ -e "$TEST_ROOT/etc/apk/cache/installed" ] || args="--no-cache $args"

	retcode=1
	if [ "$run_found" = "yes" ]; then
		# shellcheck disable=SC2086 # $args needs to be word splitted
		$APK --allow-untrusted --simulate --root-tmpfs=no $args > "$TEST_ROOT/data/output" 2>&1

		if ! cmp "$TEST_ROOT/data/output" "$TEST_ROOT/data/expected" > /dev/null 2>&1; then
			fail=$((fail+1))
			echo "FAIL: $test"
			diff -ru "$TEST_ROOT/data/expected" "$TEST_ROOT/data/output"
		else
			retcode=0
		fi
	fi

	rm -rf "$TEST_ROOT"
	return $retcode
}

TEST_TO_RUN="$*"

fail=0
pass=0
for test in ${TEST_TO_RUN:-solver/*.test}; do
	if (run_test "$test"); then
		pass=$((pass+1))
	else
		fail=$((fail+1))
	fi
done

if [ -z "$TEST_TO_RUN" ]; then
	total=$((fail+pass))
	if [ "$fail" != "0" ]; then
		echo "FAIL: $fail of $total test cases failed"
	else
		echo "OK: all $total solver test cases passed"
	fi
fi
[ "$fail" = 0 ] || exit 1
exit 0
