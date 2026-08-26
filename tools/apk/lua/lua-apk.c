/* src/lua-apk.c - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2005-2008 Natanael Copa <n@tanael.org>
 * Copyright (C) 2008-2011 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#include <lua.h>
#include <lualib.h>
#include <lauxlib.h>

#include "apk_blob.h"
#include "apk_database.h"
#include "apk_defines.h"
#include "apk_version.h"

#define LIBNAME "apk"
#define APK_DB_META "apk_database"
#define APK_IPKG_META "apk_installed_package"

#if LUA_VERSION_NUM < 502
# define luaL_newlib(L,l) (lua_newtable(L), luaL_register(L,NULL,l))
#endif

struct flagmap {
	const char *name;
	int flag;
};

struct flagmap opendb_flagmap[] = {
	{"read",                APK_OPENF_READ},
	{"write",               APK_OPENF_WRITE},
	{"create",              APK_OPENF_CREATE},
	{"no_installed",        APK_OPENF_NO_INSTALLED},
	{"no_scripts",          APK_OPENF_NO_SCRIPTS},
	{"no_world",            APK_OPENF_NO_WORLD},
	{"no_sys_repos",        APK_OPENF_NO_SYS_REPOS},
	{"no_installed_repo",   APK_OPENF_NO_INSTALLED_REPO},
	{"cache_write",         APK_OPENF_CACHE_WRITE},
	{"no_autoupdate",       APK_OPENF_NO_AUTOUPDATE},
	{"no_cmdline_repos",    APK_OPENF_NO_CMDLINE_REPOS},
	{"usermode",            APK_OPENF_USERMODE},
	{"allow_arch",          APK_OPENF_ALLOW_ARCH},
	{"no_repos",            APK_OPENF_NO_REPOS},
	{"no_state",            APK_OPENF_NO_STATE},
	{NULL, 0}
};

/* implemented as luaL_typerror until lua 5.1, dropped in 5.2
 * (C) 1994-2012 Lua.org, PUC-Rio. MIT license
 */
static int typerror (lua_State *L, int narg, const char *tname) {
	const char *msg = lua_pushfstring(L, "%s expected, got %s",
					  tname, luaL_typename(L, narg));
	return luaL_argerror(L, narg, msg);
}

static apk_blob_t check_blob(lua_State *L, int index)
{
	apk_blob_t blob;
	size_t len;
	blob.ptr = (char *)luaL_checklstring(L, index, &len);
	blob.len = len;
	return blob;
}

/* version_validate(verstr) */
/* returns boolean */
static int Pversion_validate(lua_State *L)
{
	apk_blob_t ver = check_blob(L, 1);
	lua_pushboolean(L, apk_version_validate(ver));
	return 1;
}

/* version_compare(verstr1, verstr2)
   returns either '<', '=' or '>'
*/
static int Pversion_compare(lua_State *L)
{
	apk_blob_t a, b;
	a = check_blob(L, 1);
	b = check_blob(L, 2);
	lua_pushstring(L, apk_version_op_string(apk_version_compare(a, b)));
	return 1;
}

/* version_is_less(verstr1, verstr2)
   returns whether version is '<'
*/
static int Pversion_is_less(lua_State *L)
{
	apk_blob_t a, b;
	a = check_blob(L, 1);
	b = check_blob(L, 2);
	lua_pushboolean(L, apk_version_match(a, APK_VERSION_LESS, b));
	return 1;
}

//static getfield(lua_State *L, const char *key)
//{
static const char *get_opt_string_field(lua_State *L, int index,
				        const char *key, const char *def)
{
	const char *value;
	lua_getfield(L, index, key);
	value = luaL_optstring(L, -1, def);
	lua_pop(L, 1);
	return value;
}

static void set_string_field(lua_State *L, int index, const char *key,
			     const char *value)
{
	lua_pushstring(L, key);
	lua_pushstring(L, value);
	lua_settable(L, index);
}

static void set_blob_field(lua_State *L, int index, const char *key,
			   const apk_blob_t value)
{
	lua_pushstring(L, key);
	lua_pushlstring(L, value.ptr, value.len);
	lua_settable(L, index);
}

static lua_Integer get_opt_int_field(lua_State *L, int index, const char *key, lua_Integer def)
{
	lua_Integer value;
	lua_getfield(L, index, key);
	value = luaL_optinteger(L, -1, def);
	lua_pop(L, 1);
	return value;
}

static void set_int_field(lua_State *L, int index, const char *key, lua_Integer value)
{
	lua_pushstring(L, key);
	lua_pushinteger(L, value);
	lua_settable(L, index);
}

static int get_boolean_field(lua_State *L, int index, const char *key)
{
	int value;
	lua_getfield(L, index, key);
	value = lua_toboolean(L, -1);
	lua_pop(L, 1);
	return value;
}

static int get_ctx(lua_State *L, int i, struct apk_ctx *o)
{
	struct flagmap *f;
	apk_string_array_add(&o->arch_list, (char *)get_opt_string_field(L, i, "arch", NULL));
	o->root = (char *)get_opt_string_field(L, i, "root", NULL);
	o->repositories_file = (char *)get_opt_string_field(L, i, "repositories_file", NULL);
	o->keys_dir = (char *)get_opt_string_field(L, i, "keys_dir", NULL);
	o->lock_wait = get_opt_int_field(L, i, "lock_wait", 0);
	for (f = opendb_flagmap; f->name != NULL; f++)
		if (get_boolean_field(L, i, f->name))
			o->open_flags |= f->flag;
	return 0;
}

static struct apk_database *checkdb(lua_State *L, int index)
{
	struct apk_database *db;
	luaL_checktype(L, index, LUA_TUSERDATA);
	db = (struct apk_database  *) luaL_checkudata(L, index, APK_DB_META);
	if (db == NULL)
		typerror(L, index, APK_DB_META);
	return db;
}

static int Papk_db_open(lua_State *L)
{
	struct apk_ctx *ac;
	struct apk_database *db;
	int r;

	ac = lua_newuserdata(L, sizeof(struct apk_ctx));
	apk_ctx_init(ac);
	if (lua_istable(L, 1))
		get_ctx(L, 1, ac);
	else
		ac->open_flags |= APK_OPENF_READ;

	apk_ctx_prepare(ac);
	db = lua_newuserdata(L, sizeof(struct apk_database));
	luaL_getmetatable(L, APK_DB_META);
	lua_setmetatable(L, -2);

	apk_db_init(db, ac);
	r = apk_db_open(db);
	if (r != 0)
		luaL_error(L, "apk_db_open() failed");
	return 1;
}

static int Papk_db_close(lua_State *L)
{
	struct apk_database *db = checkdb(L, 1);
	apk_db_close(db);
	apk_ctx_free(db->ctx);
	return 0;
}


static int push_package(lua_State *L, struct apk_package *pkg)
{
	if (pkg == NULL) {
		lua_pushnil(L);
		return 1;
	}
	lua_newtable(L);
	set_string_field(L, -3, "name", pkg->name->name);
	set_blob_field(L, -3, "version", *pkg->version);
	set_blob_field(L, -3, "arch", *pkg->arch);
	set_blob_field(L, -3, "license", *pkg->license);
	set_blob_field(L, -3, "origin", *pkg->origin);
	set_blob_field(L, -3, "maintainer", *pkg->maintainer);
	set_blob_field(L, -3, "url", *pkg->url);
	set_blob_field(L, -3, "description", *pkg->description);
	set_blob_field(L, -3, "commit", *pkg->commit);
	set_int_field(L, -3, "installed_size", pkg->installed_size);
	set_int_field(L, -3, "size", pkg->size);
	return 1;
}

static int Papk_who_owns(lua_State *L)
{
	struct apk_database *db = checkdb(L, 1);
	const char *path = luaL_checkstring(L, 2);
	struct apk_package *pkg = apk_db_get_file_owner(db, APK_BLOB_STR(path));
	return push_package(L, pkg);
}

static int Papk_exists(lua_State *L)
{
	struct apk_database *db = checkdb(L, 1);
	const char *depstr = luaL_checkstring(L, 2);
	struct apk_dependency dep;
	struct apk_package *pkg;
	apk_blob_t blob = APK_BLOB_STR(depstr);
	apk_blob_pull_dep(&blob, db, &dep, true);

	if (APK_BLOB_IS_NULL(blob) || blob.len > 0)
		goto ret_nil;

	pkg = apk_pkg_get_installed(dep.name);
	if (pkg == NULL)
		goto ret_nil;

	if (apk_dep_analyze(NULL, &dep, pkg) & APK_DEP_SATISFIES)
		return push_package(L, pkg);

ret_nil:
	lua_pushnil(L);
	return 1;
}

// Iterator of all installed packages
struct apk_installed_package_iterator {
	struct list_head *end;
	struct apk_installed_package *node;
};

static int iterate_installed(lua_State *L)
{
	struct apk_installed_package_iterator *i;
	struct apk_installed_package *ipkg;
	i = (struct apk_installed_package_iterator *)lua_touserdata(L, lua_upvalueindex(1));
	ipkg = i->node;

	if (&ipkg->installed_pkgs_list == i->end)
		return 0;

	i->node = list_entry(ipkg->installed_pkgs_list.next,
			  typeof(*ipkg), installed_pkgs_list);
	return push_package(L, ipkg->pkg);

}
static int Pinstalled(lua_State *L)
{
	struct apk_database *db = checkdb(L, 1);
	struct apk_installed_package_iterator *i;

	i = (struct apk_installed_package_iterator *) lua_newuserdata(L, sizeof(*i));
	i->end = &db->installed.packages;
	i->node = list_entry((&db->installed.packages)->next,
			   struct apk_installed_package,
			   installed_pkgs_list);

	lua_pushcclosure(L, iterate_installed, 1);
	return 1;
}

static const luaL_Reg reg_apk_methods[] = {
	{"version_validate",	Pversion_validate},
	{"version_compare",	Pversion_compare},
	{"version_is_less",	Pversion_is_less},
	{"db_open",		Papk_db_open},
	{"who_owns",		Papk_who_owns},
	{"exists",		Papk_exists},
	{"is_installed",	Papk_exists},
	{"installed",		Pinstalled},
	{NULL,		NULL}
};

static int db_create_meta(lua_State *L)
{
	luaL_newmetatable(L, APK_DB_META);
	lua_newtable(L);

	lua_setfield(L, -2, "__index");
	lua_pushcfunction(L, Papk_db_close);
	lua_setfield(L, -2, "__gc");
	return 1;
}

LUALIB_API int luaopen_apk(lua_State *L)
{
	db_create_meta(L);
	luaL_newlib(L, reg_apk_methods);
	lua_pushvalue(L, -1);
	lua_setglobal(L, LIBNAME);

	lua_pushliteral(L, "version");
	lua_pushliteral(L, APK_VERSION);
	lua_settable(L, -3);

	return 1;
}

