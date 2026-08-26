man_filenames = [
    'apk.8.scd',
    'apk-keys.5.scd',
    'apk-package.5.scd',
    'apk-protected_paths.5.scd',
    'apk-repositories.5.scd',
    'apk-v2.5.scd',
    'apk-v3.5.scd',
    'apk-world.5.scd',
]
foreach a : applets
	man_filenames += [ 'apk-' + a + '.8.scd' ]
endforeach

man_files = files(man_filenames)

if scdoc_dep.found()
    scdoc_prog = find_program(scdoc_dep.get_variable(pkgconfig: 'scdoc'), native: true)
    sh = find_program('sh', native: true)
    mandir = get_option('mandir')

    foreach filename : man_filenames
        topic = '@0@'.format(filename).split('.')[-3].split('/')[-1]
        section = '@0@'.format(filename).split('.')[-2]
        output = '@0@.@1@'.format(topic, section)

        custom_target(
            output,
            input: files(filename),
            capture: true,
            output: output,
            command: [
                sh, '-c', '@0@ < @INPUT@'.format(scdoc_prog.full_path())
            ],
            install: true,
            install_dir: '@0@/man@1@'.format(mandir, section)
        )
    endforeach
endif
