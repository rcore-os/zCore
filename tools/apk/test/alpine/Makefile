-include conf.mk
.PHONY:

repos	:= repo1 repo2

tmproot := /tmp/apk-test
testroot = $(tmproot)/$(basename $@)

# Absolute path to QEMU user-mode emulation binary to be copied into every
# $testroot before running test. This is used when running root-tests in
# emulation using QEMU and binfmt (apk --root uses chroot to run pre/post
# install scripts).
QEMU_EMULATOR	?=
SUDO		?= sudo
SYSREPO		?= https://dl-cdn.alpinelinux.org/alpine/edge/main
APK		?= ../../src/apk
LD_LIBRARY_PATH	?= ../../src

export APK LD_LIBRARY_PATH SYSREPO

all: alpine-tests

$(repos):
	@echo "Building $@"
	@cd "$@" && for i in */APKBUILD; do \
		cd $${i%/*} && REPODEST="$$PWD/../.." abuild -r || exit 1
	done

clean:
	rm -rf $(addsuffix /*.apk,$(repos)) $(tmproot) *.ok *.out *.stamp

repos: repos.stamp

repos.stamp: $(repos)
	@touch $@

%.ok: %.sh
	@echo -n "$<"; \
	rm -rf "$(testroot)" ;\
	mkdir -p "$(testroot)"; \
	if [ -n "$(QEMU_EMULATOR)" ]; then \
		install -Dm755 "$(QEMU_EMULATOR)" "$(testroot)$(QEMU_EMULATOR)"; \
	fi; \
	if ! $(SUDO) LD_LIBRARY_PATH="$(LD_LIBRARY_PATH)" APK="$(APK) --keys-dir /etc/apk/keys" ROOT="$(testroot)" SYSREPO="$(SYSREPO)" sh -ex ./$< > $(basename $@).out 2>&1; then\
		echo " FAIL";\
		printf '%s\n' "------------- output -------------" >&2;\
		cat $(basename $@).out >&2;\
		printf '%s\n' "----------------------------------" >&2;\
		exit 1;\
	fi ;\
	echo " OK" ;\
	$(SUDO) rm -rf $(testroot); \
	touch $@

alpine-tests: repos.stamp
	@echo "== Testing `$(APK) --version` (tests that require root permission) =="
	@failed=0; for i in test*.sh; do \
		rm -f $${i%.sh}.ok ;\
		$(MAKE) --no-print-directory $${i%.sh}.ok \
			SYSREPO=$(SYSREPO) \
			QEMU_EMULATOR=$(QEMU_EMULATOR) || failed=1; \
	done; \
	return $$failed
