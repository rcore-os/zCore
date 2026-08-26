#!/bin/sh

case "$1" in
ok)
	echo "hello"
	echo "stderr text" 1>&2
	sleep 0.2
	echo "hello again"
	echo "stderr again" 1>&2
	exit 0;;
fail)
	echo "hello"
	echo "stderr text" 1>&2
	exit 10;;
esac

exit 1
