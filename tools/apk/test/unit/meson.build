cmocka_dep = dependency('cmocka', required: get_option('tests'))

if cmocka_dep.found()

unit_test_src = [
	'blob_test.c',
	'io_test.c',
	'package_test.c',
	'process_test.c',
	'repoparser_test.c',
	'version_test.c',
	'main.c'
]

unit_test_exe = executable('unit_tests',
	files(unit_test_src),
	install: false,
	dependencies: [
		cmocka_dep,
		libapk_dep,
		libfetch_dep.partial_dependency(includes: true),
		libportability_dep.partial_dependency(includes: true),
	],
)

test('unit_tests', unit_test_exe,
	env : [
		'CMOCKA_MESSAGE_OUTPUT=XML',
		'CMOCKA_XML_FILE=' + meson.current_build_dir() + '/%g.junit.xml'
	],
	suite: 'unit',
	workdir: meson.current_source_dir(),
	priority: 200)

endif
