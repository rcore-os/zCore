libfetch_src = [
	'common.c',
	'fetch.c',
	'http.c',
	'openssl-compat.c'
]

libfetch_cargs = [
	'-DCA_CERT_FILE="/' + apk_confdir / 'ca.pem"',
	'-DCA_CRL_FILE="/' + apk_confdir / 'crl.pem"',
	'-DCLIENT_CERT_FILE="/' + apk_confdir / 'cert.pem"',
	'-DCLIENT_KEY_FILE="/' + apk_confdir / 'cert.key"',
]

# libfetch en este árbol usa OpenSSL para TLS. En entornos donde no está
# disponible (p. ej. sysroot solo con mbedtls), compilamos libfetch en modo
# HTTP-only: define FETCH_NO_TLS y excluye openssl-compat.
openssl_dep = dependency('openssl', required: false)
if not openssl_dep.found()
	libfetch_src = [
		'common.c',
		'fetch.c',
		'http.c',
	]
	libfetch_cargs += ['-DFETCH_NO_TLS']
endif

libfetch = static_library(
	'fetch',
	libfetch_src,
	c_args: libfetch_cargs,
	dependencies: [
		libportability_dep.partial_dependency(compile_args: true, includes: true),
		crypto_dep.partial_dependency(compile_args: true, includes: true),
		openssl_dep,
	],
)

libfetch_dep = declare_dependency(
	link_whole: libfetch,
	include_directories: include_directories('.'),
)
