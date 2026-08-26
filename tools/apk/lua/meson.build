luaapk_src = [
	'lua-apk.c',
]


luadep_version = lua_dep.version().split('.')
lua_instver = luadep_version[0] + '.' + luadep_version[1]

luaapk = shared_module(
	'apk',
	luaapk_src,
	dependencies: [
		lua_dep,
		libapk_dep,
		apk_deps,
		libportability_dep.partial_dependency(includes: true),
	],
	install: not subproject,
	install_dir: apk_libdir / 'lua' / lua_instver,
	name_prefix: '',
	c_args: apk_cargs,
)
