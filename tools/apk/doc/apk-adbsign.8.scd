apk-adbsign(8)

# NAME

apk adbsign - sign, resign or recompress apk v3 packages or indexes

# SYNOPSIS

*apk adbsign* [<_options_>...] _files_...

# DESCRIPTION

*apk adbsign* will process specified _files_ and add or replace
the signatures in addition to recompressing it. The original compression
algorithm and level is used unless specified.

# OPTIONS

See also *apk*(8) for additional package generation options.

*--reset-signatures*
	Remove all existing signatures.
