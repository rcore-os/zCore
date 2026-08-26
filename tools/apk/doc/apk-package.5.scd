apk-package(5)

# NAME

apk package - apk package metadata fields

# DESCRIPTION

The apk package metadata contains the package info metadata substructure
and various other metadata fields.

The package info metadata structure is the portion of package metadata which
will be copied to the repository index when the package is being indexed.
These fields will be available form the index even if the package is not
installed.

The rest of the package metadata is kept in the package and installed
database. These fields are available only if the package is installed.

The remainder of the document explains each field with the notation:
*v3-field-name* (*v2-pkginfo-field-name*, *v2-index-character*).

It is mentioned explicitly if APK uses each fields for something meaningful.
Some fields are not used internally by APK and from the APK point of view
are just blobs of data associated with specified name which are meaningful
the user.

# PACKAGE NAMES AND VERSIONS

APK will often display concatenation of *name*-*version* in its verbose
output mode. The rule below on how a valid version number is defined allow
that this format can be uniquely split back to the two components by
finding the *last* occurrence of *-[0-9]*. The dash in the beginning of this
match is the splitting point: first portion is the *name* and second
portion is the *version*.

Unfortunately it is not possible to deduce if a given string is of format
*name* or *name-version* (*name* alone can also contain *-[:digit:]* in it).

# PACKAGE INFO METADATA

*name* (*pkgname*, *P*)
	Package name. This is the primary package name. The name shall
	consist only of the following characters [a-zA-Z0-9.\_+-].
	The name must start with an alphanumeric character [a-zA-Z0-9].

*version* (*pkgver*, *V*)
	Package version. The Alpine version specification originally
	followed the Gentoo package version specification.

	Currently the APK version specification is as follows:
	*number{.number}...{letter}{\_suffix{number}}...{~hash}{-r#}*

	Each *number* component is a sequence of digits (0-9).

	The *letter* portion can follow only after end of all the numeric
	version components. The *letter* is a single lower case letter (a-z).

	Optionally one or more *\_suffix{number}* components can follow.
	The list of valid suffixes (and their sorting order) is:
	*alpha*, *beta*, *pre*, *rc*, <no suffix>, *cvs*, *svn*, *git*, *hg*, *p*.

	This can be followed with an optional *{~hash}* to indicate a commit
	hash from where it was built. This can be any length string of
	lower case hexadecimal digits (0-9a-f).

	Finally an optional package build component *-r{number}* can follow.

*hashes* (*C*)
	Hash of the package meta data. This field is present only in
	the index copy of the package info.

	APK uses this fields in multiple ways:
	- authenticate and verify the package against an index
	- determine if same identical package is available from multiple
	  repositories
	- make package filename unique when storing a copy in the package
	  cache

*description* (*pkgdesc*, *T*)
	The description is a single line describing the package.
	APK displays this string in various command querying information about
	the package, repository or installed database.

*arch* (*arch*, *A*)
	Package architecture for which the package was built. Currently apk
	uses the following default architectures:
	- noarch
	- aarch64
	- arc700
	- archs
	- armeb
	- armel
	- armhf
	- armv7
	- mips
	- mipsel
	- mips64
	- mips64el
	- ppc
	- ppc64
	- ppc64le
	- riscv32
	- riscv64
	- s390x
	- sh2eb
	- sh3
	- sh4
	- loongarchx32
	- loongarch64
	- wasi32
	- wasi64
	- x86
	- x86_64

	The arch field can be part of the repository download URL. See
	*apk-mkndx*(8) *--pkgname-spec* for additional details.

	Package is not eligible for installation unless the arch matches
	one of the values in *etc/apk/arch*.

*license* (*license*, *L*)
	Package license. This is informative field for the user and APK does
	not validate or use this field internally. It is recommended to use
	standard license descriptors such as SPDX.

*origin* (*origin*, *o*)
	Package's source package name. APK uses this field as follows:
	- If two separate binary packages share same source package, APK allows
	  overwriting the package to overwrite files from another package. This
	  serves the purpose of moving files from one subpackage to another.
	- Several query commands allow printing or matching the original package name.
	- Indexing command (when updating index incrementally) uses this field
	  determine when to delete old package (that is to delete subpackages
	  that no longer exist).

*maintainer* (*maintainer*, *m*)
	Package's maintainer information. Usually the name and email address.

*url* (*url*, *U*)
	Package URL. A link to website containing information about the package.

*repo-commit* (*commit*, *c*)
	Repository commit hash from which the package was built from.

*build-time* (*builddate*, *t*)
	UNIX timestamp when the package was built. Apk fetch can filter packages
	to download based on the build time. This is useful to download incremental
	repository snapshots.

*installed-size* (*size*, *I*)
	Estimate of how much disk space is required when the package is installed.
	APK displays this information in various places, and based the commit
	transaction disk usage changed on this information.

	Packages with the installed size being zero as meta packages that do not
	have any other data than indexed data. APK may choose to not download the
	package and handle everything based on the data available in the index.

*file-size* (*S*)
	This field is present meaningful only in the repository index copy of
	the package info. APK index will fill this field at indexing time with the
	size of the package file (.apk). Technically this field should be a repository
	index specific field, and such change might be done in the future.

*provider-priority* (*provider_priority*, *k*)
	This determines the default installation priority for the non-versioned
	package names the packages lists in the *provides* field. By default
	a non-versioned provides will not be selected automatically for installation.
	But specifying *provider-priority* enables this automatic selection, and is
	used to determine which of the packages to install in case multiple packages
	provide the same non-versioned package name.

*depends* (*depend*, *D*)
	List of dependencies for the package. Installing this package will
	require APK to first satisfy the list of all its dependencies.

	The dependencies are used by various APK components:
	- The solver will try to find a solution that all package dependencies
	  are satisfied (as well as the world dependencies)
	- When apk is committing changes to the file system, it will install
	  or remove packages in such order that all dependencies of the package
	  will be satisfied (assuming there are no circular dependencies)
	- When apk runs the package trigger scripts, they will be ordered
	  so that the triggers of all dependencies before running the trigger
	  for this package

*provides* (*provides*, *p*)
	List of package names (and optionally its version) this package
	provides in addition to its primary name and version. The provided
	name can contain additionally characters: comma (,), brackets ([]),
	colons (:) and slashes (/) in the name.	This allows using namespaces
	for automatically generated names.

	If the provided name contains a version number:
	- the solver will treat it as-if a real package with the provided
	  name is installed
	- the package becomes automatically selectable by anything depending
	  on the provided name
	- the package will automatically become the single possible owner
	  for the provided name
	- the package will automatically conflict with any package with
	  the same primary or provided package name

	If the provided name does not include version:
	- the package is not automatically selectable for installation
	  by that fact that there is a dependency on the provided name
		- specifying *provides_priority* will allow automatic selection
		- otherwise user is expected to manually select one of the
		  concrete package names in world which allows selection
	- the package is not considered to own provided name
	- multiple packages provided the same name without a version are
	  allowed to be installed simultaneously
	- apk internally considers a package name with only non-versioned
	  providers as a "virtual package name"

*replaces* (*r*)
	List of package names this package is allowed to replace files from.
	Normally apk treats it as an error if multiple packages contain the
	same file. Specifying a replaces declaration allows the package to
	silently overwrite files from the listed packages.

*install-if* (*install_if*, *i*)
	APK will automatically select and install the package if all of
	the install-if dependencies are satisfied. There should be at least
	two dependencies in *install_if* dependencies, and one of them must
	have a equality (*=*) operator.

	Typical use case is that there is a global repository meta package
	e.g. *docs*. And then there are multiple packages that have a subpackage
	like *package-doc*. These *-doc* packages can then have a *install-if*
	rule to get automatically installed if such as "*package=$name-$ver docs*"
	to install the documentation package automatically if the main package
	and the documentation meta package is installed.

*recommends*
	List of dependencies recommended to install along with this package.
	This is currently not used by APK for anything, but is stored, dumped
	and queryable.

*layer*
	An integer specifying the database layer this package installs to:
	- *root* (0) is the default and indicates the normal file system
	- *uvol* (1) indicates that the package contains an uvol image and
	  the uvol volume manager should be used to install the images

	In addition to controlling where the package content goes, this also
	affects the installed database where the metadata of these packages
	go. Each layer has a separate installed database.

*tags*
	List of tags that this package will match against. Apk does not do
	anything with the tags, but the distribution vendors can define their
	own tags to associate custom metadata with the package. The tags can
	be queried and dumped using the *apk-query*(8) applet.
	Each tag consists of the following characters [a-zA-Z0-9.\_+-,:/\[\]=].
	Custom tags should contain a distribution or vendor specific prefix
	such as e.g. "alpine:".

# PACKAGE METADATA

*info*
	This is the logical structure containing the package info metadata
	as defined in the previous section.

*paths*
	This contains listing of all the paths and files along with the file
	specific metadata (owner, permissions, xattrs, content hashes).

*scripts*
	Scripts contains the executable files (usually shell scripts) that
	are executed before or after package installation, removal, upgrade
	as well as to handle trigger conditions.

	Currently defined script types and their arguments:
	- trigger <matched-trigger>...
	- pre-install <new-version>
	- post-install <new-version>
	- pre-deinstall <old-version>
	- post-deinstall <old-version>
	- pre-upgrade <new-version> <old-version>
	- post-upgrade <new-version> <old-version>

	See also the ENVIRONMENT section in *apk*(8) for the environment variables.

*triggers*
	List of directory globs. APK will execute the trigger script with
	list of matched directories when any action (package installation,
	removal) has modified content of that directory. When package is
	being fixed or installed it will get list of all matching directories.

	Trigger globs may start with *+*, which means that the path should
	only be passed to the trigger script when the directory was modified
	during the transaction. It does not affect whether the trigger is
	invoked or not. Without the prefix, the path will also be passed
	when present in the system and the package providing the trigger
	script is updated or reinstalled.

*replaces-priority*
	If two packages both contain the same file, and they both have replaces
	directive allow them to overwrite packages. This priority determines
	which packages file is takes precedence.

# SEE ALSO

*abuild*(1), *apk*(8), *apk-v2*(5), *apk-v3*(5)
