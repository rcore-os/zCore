scdocs-y += \
	apk-cache.5 \
	apk-keys.5 \
	apk-package.5 \
	apk-repositories.5 \
	apk-v2.5 \
	apk-v3.5 \
	apk-world.5 \
	apk.8 \
	apk-adbdump.8 \
	apk-adbgen.8 \
	apk-adbsign.8 \
	apk-add.8 \
	apk-audit.8 \
	apk-cache.8 \
	apk-convndx.8 \
	apk-convdb.8 \
	apk-del.8 \
	apk-dot.8 \
	apk-extract.8 \
	apk-fetch.8 \
	apk-fix.8 \
	apk-index.8 \
	apk-info.8 \
	apk-list.8 \
	apk-manifest.8 \
	apk-mkndx.8 \
	apk-mkpkg.8 \
	apk-policy.8 \
	apk-protected_paths.5 \
	apk-query.8 \
	apk-stats.8 \
	apk-update.8 \
	apk-upgrade.8 \
	apk-verify.8 \
	apk-version.8

install:
	for page in $(scdocs-y); do \
		section=$${page#*.}; \
		$(INSTALLDIR) $(DESTDIR)$(MANDIR)/man$$section; \
		$(INSTALL) $(obj)/$$page $(DESTDIR)$(MANDIR)/man$$section/; \
	done
