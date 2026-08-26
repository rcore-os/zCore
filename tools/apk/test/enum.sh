#!/bin/sh

set -e

cd "$(dirname "$0")"
case "$1" in
solver)
	echo solver/*.test
	;;
shell)
	echo user/*.sh
	;;
esac
