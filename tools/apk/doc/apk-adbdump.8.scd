apk-adbdump(8)

# NAME

apk adbdump - dump v3 files in textual representation

# SYNOPSIS

*apk adbdump* [<_options_>...] _files_...

# DESCRIPTION

*apk adbdump* will dump the metadata portion of given _files_ (packages,
indexes, installeddb) to its textual representation.

The output can be:
	- yaml with annotations as comment
	- json data blob

*apk-adbgen*(8) can convert the YAML textual representation back to
the binary format with certain limitations (nested ADB and DATA blocks
are not supported).

# OPTIONS

*--format* _FORMAT_
	Specify the output format (options: *json*, *yaml*)

# SEE ALSO

*apk-adbgen*(8), *apk-package*(5), *apk-v3*(5)
