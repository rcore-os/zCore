apk-del(8)

# NAME

apk del - remove constraints from _world_ and commit changes

# SYNOPSIS

*apk del* [<_options_>...] _constraints_...

# DESCRIPTION

*apk del* removes constraints from _world_ (see *apk-world*(5)) and
commits changes to disk. This usually involves removing unneeded packages,
but may also cause other changes to the installed packages.

# OPTIONS

*apk del* supports the commit options described in *apk*(8), as well as the
following options:

*--rdepends*, *-r*
	Recursively delete all top-level reverse dependencies, too.
