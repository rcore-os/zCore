apk-cache(8)

# NAME

apk cache - manage a local cache of package files

# SYNOPSIS

*apk cache* [<_options_>...] clean

*apk cache* [<_options_>...] download [_dependency_...]

*apk cache* [<_options_>...] purge

*apk cache* [<_options_>...] sync [_dependency_...]

# DESCRIPTION

Manage local package cache by removing obsolete packages, or downloading
missing or newer packages from the repositories.

*apk cache download* will fetch package files from the repositories and store
them in the cache, which must be enabled upfront (see *apk-cache*(5)). By
default _world_ dependencies are used to determine what to download. If
_dependency_ arguments are given, they will by default replace the _world_.

*apk cache clean* will remove package files which no longer exist in any
repository index. Specifying the global option *--purge* will additionally
remove all uninstalled package on tmpfs installations, and all packages on
disk installations.

*apk cache purge* is a synonym for *clean --purge*.

*apk cache sync* performs both operations.

# OPTIONS

*--add-dependencies*
	Add the argument dependencies to _world_ dependencies when determining
	which packages to download.

*--available*, *-a*
	Selected packages to be downloaded from active repositories even if
	it means replacing or downgrading the installed package.

*--ignore-conflict*
	Ignore conflicts when resolving dependencies. This can be useful when
	pre-populating cache for creation of multiple images in one go.

*--latest*, *-l*
	Always choose the latest package by version. However, the versions
	considered are based on the package pinning. Primarily this overrides
	the default heuristic and will cause an error to displayed if all
	dependencies cannot be satisfied.

*--upgrade*, *-u*
	When caching packages which are already installed, prefer their upgrades
	rather than considering the requirement fulfilled by the current
	installation.

*--simulate*[=_BOOL_], *-s*
	Simulate the requested operation without making any changes.
