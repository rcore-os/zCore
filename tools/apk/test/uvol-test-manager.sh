#!/bin/sh

echo "uvol-test: $*"

case "$1:$2" in
write:data)
	read -r DATA
	[ "$DATA" = "Hello world!" ] || echo "uvol-test incorrect data!"
	echo "uvol-test: drained input"
	;;
write:scriptfail)
	exit 2
esac

exit 0
