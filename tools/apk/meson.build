project(
	'apk-tools',
	['c'],
	default_options : ['c_std=gnu11', 'optimization=2', 'warning_level=1'],
	version: run_command('./get-version.sh', check: true).stdout().strip(),
	meson_version: '>=0.64'
)
so_version = run_command('cat', 'SOVERSION', check: true).stdout().strip()

pkgc = import('pkgconfig')

apk_confdir = get_option('sysconfdir') / 'apk'
apk_libdir = get_option('libdir')

lua_name = 'lua' + get_option('lua_version')
lua_bin_name = get_option('lua_bin')
lua_pc_name = get_option('lua_pc')

lua_bin = find_program(lua_bin_name != '' ? lua_bin_name : lua_name, required: get_option('help'), native: true)
lua_dep = dependency(lua_pc_name != '' ? lua_pc_name : lua_name, required: get_option('lua'))

python_dep = dependency('python3', required: get_option('python'))
scdoc_dep = dependency('scdoc', version: '>=1.10', required: get_option('docs'), native: true)
zlib_dep = dependency('zlib')
libzstd_dep = dependency('libzstd', required: get_option('zstd'))

if get_option('crypto_backend') == 'openssl'
	crypto_dep = dependency('openssl')
elif get_option('crypto_backend') == 'mbedtls'
	# Keep crypto_dep as a single dependency object: some subdirs call
	# crypto_dep.partial_dependency(...).
	cc = meson.get_compiler('c')
	crypto_dep = dependency('mbedtls', required: false)
	if not crypto_dep.found()
		crypto_dep = cc.find_library('mbedtls', required: true)
	endif
	crypto_extra_deps = [ dependency('mbedcrypto', required: false) ]
	if not crypto_extra_deps[0].found()
		crypto_extra_deps = [ cc.find_library('mbedcrypto', required: true) ]
	endif
endif

apk_deps = [ crypto_dep, zlib_dep, libzstd_dep ]
if get_variable('crypto_extra_deps', []).length() > 0
	apk_deps += crypto_extra_deps
endif

add_project_arguments('-D_GNU_SOURCE', language: 'c')

# Needed for 64-bit off_t and friends on glibc.
add_project_arguments('-D_FILE_OFFSET_BITS=64', language: 'c')

# If we're a subproject we only want the static lib and not files
subproject = meson.is_subproject()

# applet configuration
applets = [
	'adbdump',
	'add',
	'audit',
	'cache',
	'del',
	'extract',
	'fetch',
	'fix',
	'info',
	'list',
	'manifest',
	'policy',
	'query',
	'update',
	'upgrade',
	'search',
	'stats',
	'verify',
	'version',
]

if not get_option('minimal')
	applets += [
		'adbgen',
		'adbsign',
		'convdb',
		'convndx',
		'dot',
		'index',
		'mkndx',
		'mkpkg',
	]
elif not get_option('tests').disabled()
	error('Tests are not supported with minimal build')
endif

subdir('doc')
subdir('portability')
if get_option('url_backend') == 'libfetch'
	subdir('libfetch')
else
	libfetch_dep = dependency('', required: false)
endif
subdir('src')
if lua_dep.found()
	subdir('lua')
endif
if python_dep.found()
	subdir('python')
endif
subdir('test')
