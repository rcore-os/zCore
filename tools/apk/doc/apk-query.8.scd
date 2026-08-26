apk-query(8)

# NAME

apk query - query information about packages by various criteria

# SYNOPSIS

*apk query* [<_options_>...] _query_...

*apk query* [<_options_>...] *--recursive* _constraints_...

# DESCRIPTION

*apk query* searches for matching packages from selected sources.

In the default mode, _query_ specifiers are interpreted as follows:
	*name{[<>~=]version}*
		Select packages by *name* and optional *version* match.
	*text*
		Select packages by selected fields matching *text*.

In the *--recursive* mode, the _constraints_ specify a list of dependencies
to satisfy and the solver algorithm is used to determine a list of packages
that fulfil these constraints.

The query executes in the following steps:
. Each _query_ string is executed independently to select candidate packages
. If *--all-matches* is not specified, the best candidate for given term
  is added to the list of result packages
. The resulting package list is sorted

# QUERY OPTIONS

The applets supporting query specifiers recognize the following options:

*--all-matches*
	Select all matched packages. By default only best match for each query
	element is selected.

*--available*
	Filter selection to available packages.

*--fields* _FIELDS_[:_REVERSE_FIELD_]
	A comma separated list of fields to include in the output. An optional
	specification to specify the field to output for the synthetic reverse
	dependency fields can be specified (*name*, *package* or *origin*).

*--format* _FORMATSPEC_
	Specify output format from *default*, *yaml* or *json*. The *default*
	format is human readable text output.

*--from* _FROMSPEC_
	Search packages from: *system* (all system sources), *repositories*
	(exclude installed database), *installed* (exclude normal repositories)
	or *none* (command-line repositories only).

*--installed*
	Filter selection to installed packages.

*--match* _FIELDS_
	A comma separated list of fields to match the query against.

*--recursive*
	Run solver algorithm with given _constraints_ to select packages.

*--summarize* _FIELD_[:_REVERSE_FIELD_]
	Produce a summary of the specified field from all matches.
	Summary is available on the following fields: *package*, *name*,
	*origin*, *depends*, *provides*, *replaces*, *install_if*,
	*recommends*, *reverse-depends*, and *reverse-install-if*.

*--upgradable*
	Filter selection to upgradable packages.

*--world*
	Include *apk-world*(5) dependencies in constraints. Implies *--recursive*.

*--orphaned*
	Filter selection to orphaned packages.

# FIELDS

The field names are all small letters for *--match* and *--fields* options
and for the machine parseable output (json and yaml). For the human readable
default format the fields are capitalized.

The following package metadata fields are available:
*name*, *version*, *description*, *arch*, *license*, *origin*, *maintainer*,
*url*, *commit*, *build-time*, *installed-size*, *file-size*, *provider-priority*,
*depends*, *provides*, *replaces*, *install-if*, *layer*, *tags*, *triggers*,
*scripts*, and *replaces-priority*.
See *apk-package*(8) *package info metadata* and *package metadata* sections
for the description of these fields.

Additionally the following fields are available:

*contents*
	File names contained in a package.

*download-url*
	Full URL to download the package from.

*owner*
	Lookup owner package for given path name. (*--match* only)

*package*
	The package identifier in format *name*-*version* (e.g.
	package-1.0-r0).

*repositories*
	List of repositories the package is available from.

*status*
	Status of an installed package. List of one or more of following
	keywords:
	- *installed*
	- *broken-files*
	- *broken-scripts*
	- *broken-xattr*

# EXAMPLES

\# search all packages starting with apk++
apk query "apk\*"

\# show owner package of sensors executable in json++
apk query --format json --match owner /usr/bin/sensors

\# show apk-tools and its dependencies in yaml++
apk query --format yaml --recursive apk-tools

\# print source packages for all packages providing cmd:apk++
apk query --match name,provides cmd:apk --fields origin

\# print source packages with specific dependency name++
apk query --match dependency so:libapk.so.2.14.9 --fields origin
