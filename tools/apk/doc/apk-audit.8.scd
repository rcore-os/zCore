apk-audit(8)

# NAME

apk audit - audit directories for changes

# SYNOPSIS

*apk audit* [<_options_>...] _directories_...

# DESCRIPTION

*apk audit* audits the system or specified directories for changes compared to
the package database.

The audit can be done against configuration files only (*--backup*) to generate
list of files needed to be stored in the overlay in run-from-tmps configuration.
Alternatively, it can audit all installed files (*--system* or *--full*) to
e.g. detect unauthorized modifications of system files.

By default, the output format is one file per line, for each modified file.
A character is printed indicating the line type, followed by a space,
then the affected path or details. The changes detected are:

|[ -
:< Database detail record
|  \+
:  On-disk detail record
|  A
:  File added
|  d
:  Directory added
|  D
:  Directory added (with non-listed files/subdirs)
|  e
:  error occurred during audit (e.g. no permissions to read file)
|  M
:  File metadata changed (uid, gid, or mode)
|  m
:  Directory metadata changed
|  U
:  File contents modified
|  X
:  File deleted
|  x
:  xattrs changed

# OPTIONS

*--backup*
	Audit configuration files only (default). The list of files to be
	audited is generated from the masks in protected_paths.d.

*--check-permissions*
	Check file permissions too. Namely, the uid, gid and file mode will
	be checked in addition to the file content.

*--details*
	Enable reporting of detail records.

*--full*
	Same as *--system*, but in addition reports all added directories and
	files. A built-in default override for protected paths is used, unless
	a *--protected-paths* is explicitly specified.

*--ignore-busybox-symlinks*
	Ignore symlinks whose target is the busybox binary.

*--packages*
	Print only the packages with changed files. Instead of the full output
	each modification, the set of packages with at least one modified file
	is printed.

	To repair all packages with modified files, one could use:
		apk audit --packages -q | xargs apk fix

*--protected-paths* _FILE_
	Use given FILE for protected paths listings. This also makes apk ignore
	the regular protected_paths.d directories.

*--recursive*, *-r*
	Descend into directories and audit them as well.

*--system*
	Audit all system files. All files provided by packages are verified
	for integrity with the exception of configuration files (listed in
	protected_paths.d). This is useful detecting unauthorized file changes.
	New files or directories are not reported.

# SEE ALSO

*apk-protected_paths*(5)
