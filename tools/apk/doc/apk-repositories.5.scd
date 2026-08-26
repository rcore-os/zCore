apk-repositories(5)

# NAME

*/etc/apk/repositories*++
*/etc/apk/repositories.d/\*.list*++
*/lib/apk/repositories.d/\*.list*++
	list of package repositories

# DESCRIPTION

*apk*(8) loads repository definitions from the above mentioned files.

The *repositories* file is first loaded. The *repositories.d* paths are
then processed if *--repositories-file* option was not used. The directories
are enumerated in the above mentioned order. Once a *.list* file of given
name is seen, any file of the same name in subsequent directories is ignored.

# FILE FORMAT

Each line follows one of the following syntax:

*\# comment*
	A comment line which is ignored.

*set \[-default\] key=value*
	Set the variable named *key* to given *value*.

	The *key* is limited to letters, numbers and the underscore (\_) character,
	and must start with a letter. The *APK_* prefixed variable names are
	reserved for built-in variables and cannot be defined by the user.

	A variable expansion is performed on the *value* before assignment.

	If the option *-default* is specified, the *key* value is not changed
	if it already exists.

	Currently the following variables are defined:
		- *APK_ARCH*, the primary architecture of the database

*ndx \[@tag\] url*++
*\[v2|v3\] \[@tag\] url \[component...\]*
	If a repository declaration omits the type field, it defaults to the type
	defined in latest *default* command, or if absent, to *v2* (or *ndx* if *url*
	ends with *.adb* or *.tar.gz*). If the format is *v3* or the *ndx* filename
	ends with *.adb* a v3 format index file required.

	The optional *@tag* can be present to indicate that the repository should
	not be used by default. Refer to *apk-world*(5) on how to enable installation
	of packages from tagged repositories.

	A variable expansion is performed on the *url* and *component* portions
	individually at the time of parsing the line.

	The *url* refers to an index file or a repository path. Currently supported
	schemes are:
		- _http://_
		- _https://_
		- _file://_
		- absolute filesystem path (must start with `/`)

	The *component* list specifies a list of repository components. If specified,
	the line is expanded to multiple URLs: one for each component, and the *component*
	is appended to the *url*. Specifying *component* with *ndx* type is not valid.

# EXAMPLES

To define a distribution provided repository list, the distribution can
ship e.g */lib/apk/repositories.d/distribution.list* with the contents:

	set -default distro_mirror=https://example.com/distro++
v3 ${distro_mirror} main community

In */etc/apk/repositories.d/00-vars.list* the mirror can be overridden with:

	set distro_mirror=https://mirror.example.com/distro

# REPOSITORY LAYOUT

If the *type* is *ndx*, the layout and path resolution is as follows:
	- *url* is a URL to the index file
	- *base_url* is *url* with last component stripped off
	- default package path: *$base_url/$name-$version.apk*

Otherwise the *type* is one of the repository types, and the path resolution
works as follows:
	- *url* is the *base_url* to the repository
	- index (*v3*) is at *$base_url/$arch/Packages.adb*
	- index (*v2*) is at *$base_url/$arch/APKINDEX.tar.gz*
	- default package path: *$base_url/$arch/$name-$version.apk*

If the index file has a *pkgname-spec* set, it is used to construct the package path.
Otherwise the default package path based on above rules is used.

# CREATING INDEXES AND REPOSITORIES

See *apk-mkndx*(8) on how to create index files and *apk-adbsign*(8) on modifying
signatures.

For the legacy index format refer to *apk-index*(8) for index files, *abuild-keygen*(1)
for information about generating keys, *abuild-sign*(1) for information about using
these keys to sign files.

*apk-verify*(8) can verify both new and old style index and package files.

# CLIENT INDEX UPDATING

*apk*(8) fetches and stores the index for each repository at a local cache
in */etc/apk/cache* or */var/cache/apk*. Refer to *apk-cache*(5) for more details.

Refer to *apk-keys*(5) on how the client needs to be configured for verifying
the index signatures.
