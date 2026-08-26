py_mod = import('python')

py_inst = py_mod.find_installation('python3', pure: false)

py_ext = py_inst.extension_module(
	'apk',
	sources: files('apk_module.c'),
	dependencies: [
		libapk_dep,
		python_dep,
		apk_deps,
		libportability_dep.partial_dependency(includes: true),
	],
	install: true,
	gnu_symbol_visibility: 'hidden',
)

env = environment()
env.append('PYTHONPATH', meson.current_build_dir())
env.append('TESTS_BUILDDIR', meson.current_build_dir())
test('apk-python-tests',
	py_inst,
	args : ['-m', 'unittest', 'discover', '-s', 'tests'],
	workdir : meson.current_source_dir(),
	env : env,
	depends : py_ext,
)
