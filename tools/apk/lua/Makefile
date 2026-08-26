ifeq ($(LUA),no)
LUAAPK			?= no
else ifneq ($(LUA),)
LUAAPK			?= yes
else
$(error Lua interpreter not found. Please specify LUA interpreter, or use LUA=no to build without help.)
endif

# Lua module
ifeq ($(LUAAPK),yes)
LUA_VERSION		?= 5.3
LUA_PC			?= lua$(LUA_VERSION)
LUA_LIBDIR		?= /usr/lib/lua/$(LUA_VERSION)
shlibs-y		+= apk.so
apk.so-objs		:= lua-apk.o
CFLAGS_lua-apk.o	:= -DAPK_VERSION=\"$(VERSION)\" -Iportability -Isrc \
			   $(shell $(PKG_CONFIG) $(LUA_PC) --cflags)
apklua_so		:= $(obj)/apk.so
install-apklua_so	:= $(INSTALLDIR) $(DESTDIR)$(LUA_LIBDIR) && \
			   $(INSTALL) $(apklua_so) $(DESTDIR)$(LUA_LIBDIR)
endif

install: $(apklua_so)
	$(install-apklua_so)
