apk-add(8)

# NAME

apk add - add or update constraints to _world_ and commit changes

# SYNOPSIS

*apk add* [<_options_>...] [_constraints_|_file_]...

# DESCRIPTION

*apk add* adds or updates given constraints to _world_ (see *apk-world*(5))
and commit changes to disk. This usually involves installing new packages,
but may also cause other changes to the installed packages.

To upgrade individual packages without modifying _world_ use *apk-fix*(8).

By default *apk* tries to select a set of packages so that all constraints
are satisfied. This means that some packages can be held back in an older
version if there is direct or implicit dependencies to the older version.

If a _file_ is added, a constraint against the package identity hash will
be add. This constraint ensure that the exact file gets installed and kept.
To later upgrade or downgrade back to a repository version, use
*apk upgrade --available*.

# OPTIONS

*apk add* supports the commit options described in *apk*(8), as well as the
following options:

*--initdb*
	Initialize a new package database.

*--latest*, *-l*
	Always choose the latest package by version. However, the versions
	considered are based on the package pinning. Primarily this overrides
	the default heuristic and will cause an error to displayed if all
	dependencies cannot be satisfied.

*--no-chown*
	Deprecated alias for --usermode.

*--upgrade*, *-u*
	Upgrade _packages_ and their dependencies. Normally *apk add* will
	avoid changing installed package unless it is required by the newly
	added _packages_ or their dependencies. This changes the default
	preference to upgrading the package to the latest installable version.

*--usermode*
	Create usermode database with --initdb. In usermode, apk will operate
	in such a way that root access is not required. Currently this implies:
	- checking that we are running as non-root
	- not changing file owner or group
	- not setting system xattrs

	This implies that the installation might not be fully functional.
	However, this is useful mode for testing purposes or to create
	chroot where some specific applications can run as non-root.

*--virtual*, *-t* _NAME_
	Create virtual package _NAME_ with given dependencies. This new package
	will get the _packages_ as dependencies instead of _world_. Finally the
	_NAME_ is added to _world_. An optional version specifier for the virtual
	package can be given via syntax _NAME_=_VERSION_. The version defaults
	to synthesized version based on time.

	One can use this to ensure that selected set of packages are installed,
	and later the temporary modification be undone with *apk-del*(8) _NAME_
	without removing packages that were installed earlier.
