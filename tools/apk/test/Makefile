-include conf.mk

.PHONY:

APK		?= $(realpath ../src/apk)
LD_LIBRARY_PATH	?= $(realpath ../src)
APK_CONFIG = /dev/null

export APK LD_LIBRARY_PATH APK_CONFIG

all: tests

test tests check:
	@echo "== Testing `$(APK) --version` =="
	@for i in $(wildcard user/*.sh solver.sh); do \
		./$$i > test.log 2>&1 || { echo "FAIL: $$i"; cat test.log; rm -f test.log; exit 1; }; \
		echo "OK: $$i"; \
	done
	@rm -f test.log

