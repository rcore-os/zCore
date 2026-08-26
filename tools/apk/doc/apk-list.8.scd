apk-list(8)

# NAME

apk list - list packages matching a pattern or other criteria

# SYNOPSIS

*apk list* [<_options_>...] _pattern_...

# DESCRIPTION

*apk list* searches package indices for packages matching the given patterns
and prints any matching packages.

The patterns are matched with *fnmatch*(3), which behaves similarly to shell
globbing.

# OPTIONS

In addition to the global options (see *apk*(8)), and query options
(see *apk-query*(8)) the following options are supported:

*--available*, *-a*
	Consider only available packages.

*--depends*, *-d*
	List packages by dependency.

*--installed*, *-I*
	Consider only installed packages.

*--manifest*
	List installed packages in format `<name> <version>`.

*--origin*, *-o*
	List packages by origin.

*--orphaned*, *-O*
	Consider only orphaned packages.

*--providers*, *-P*
	List packages by provider.

*--upgradable*, *--upgradeable*, *-u*
	Consider only upgradable packages.
