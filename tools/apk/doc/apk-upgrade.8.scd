apk-upgrade(8)

# NAME

apk upgrade - upgrade installed packages

# SYNOPSIS

*apk upgrade* [<_options_>...] [<_packages_>...]

# DESCRIPTION

*apk upgrade* upgrades installed packages to the latest version available from
configured package repositories (see *apk-repositories*(5)). When no packages
are specified, all packages are upgraded if possible. If list of packages is
provided, only those packages are upgraded along with needed dependencies.

If dependencies mentioned in *--preupgrade-depends* (see *apk*(8)) match
an upgradable package, a preupgrade step is performed. During the preupgrade
step, the installed packages matching the preupgrade dependencies are upgraded
if possible - their dependencies are enforced (but not upgraded unless required
by the dependencies in the new package).

An implicit preupgrade dependency is generated for the package owning the apk
executable.

# OPTIONS

*apk upgrade* supports the commit options described in *apk*(8), as well as the
following options:

*--available*, *-a*
	Reset all packages to versions available from current repositories.
	This resets all versioned dependencies in _world_ (see *apk-world*(5)).
	Additionally, packages are selected from active repositories if possible
	even if it means replacing or downgrading the package.

	This is useful to reset system against new set of packages after updating
	repositories.

*--ignore*
	Upgrade all other packages than the ones listed. This inverts the given
	package name list to mean packages that should not be upgraded.

*--latest*, *-l*
	Always choose the latest package by version. However, the versions
	considered are based on the package pinning. Primarily this overrides
	the default heuristic and will cause an error to displayed if all
	dependencies cannot be satisfied.

*--preupgrade*[=_BOOL_]
	If turned off, disables the preupgrade step.

*--preupgrade-only*
	Perform only the preupgrade.

*--prune*
	Prune the _world_ by removing packages which are no longer available
	from any configured repository.
