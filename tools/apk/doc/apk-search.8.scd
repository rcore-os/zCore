apk-search(8)

# NAME

apk search - search for packages by name or description

# SYNOPSIS

*apk search* [<_options_>...] _pattern_...

# DESCRIPTION

*apk search* searches all repositories for packages matching at least one
pattern. If no pattern is given, it lists all packages in the configured
repositories (see *apk-repositories*(5)). A pattern matches if it is a
case-insensitive substring of the package name.

# OPTIONS

In addition to the global options (see *apk*(8)), and query options
(see *apk-query*(8)) the following options are supported:

*--all*, *-a*
	Print all matching package versions. By default, *apk* only shows the
	latest version.

*--description*, *-d*
	Also search for _pattern_ in the package description. By default, *apk*
	does not search package descriptions.

*--exact*, *-e*, *-x*
	Match package names exactly.

*--has-origin*
	Match by package origin. Shows all packages whose base package name
	matches _pattern_ exactly. Implies *--all* and *--exact*.

*--origin*, *-o*
	Print base package name.

*--rdepends*, *-r*
	Print reverse dependencies (other packages which depend on the
	package).
