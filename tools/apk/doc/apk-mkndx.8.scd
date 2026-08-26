apk-mkndx(8)

# NAME

apk mkndx - create apkv3 repository index file from packages

# SYNOPSIS

*apk mkndx* [<_options_>...] _packages_...

# DESCRIPTION

*apk mkndx* creates a repository index from a list of package files. See
*apk-repositories*(5) for more information on repository indices.

# OPTIONS

*--description*, *-d* _TEXT_
	Add a description to the index. Upstream, this is used to add version
	information based on the git commit SHA of aports HEAD at the time of
	index generation.

*--filter-spec* _PKGNAME_SPEC_
	Filter previous index only. Each argument should be _PKGNAME_SPEC_ formatted
	name of a package to include from the index. This can be used to create
	a subset of existing index.

*--hash* _HASH_
	Use _HASH_ as the algorithm for apk v3 integrity. Currently supported:
	- sha256 (default)
	- sha256-160

	The *sha256-160* is allowed to generate index compatible with old
	prereleases of apkv3 that do no handle longer hashes correctly.

*--index*, *-x* _INDEX_
	Read an existing index from _INDEX_ to speed up the creation of the new
	index by reusing data when possible.

*--output*, *-o* _FILE_
	Output generated index to _FILE_.

*--pkgname-spec* _PKGNAME_SPEC_
	Specify package name specification for downloading the packages.
	APK will construct the download URL relative to index file by expanding
	this specification with package specific values.

	If the specification contains :// it is considered an absolute URL instead
	of relative. This is not recommended for public repositories as using
	absolute package name specification would prevent mirroring. However, this
	is useful in build environment to create a subset of an index and have it
	refer to packages in another repository.

	If not specified, the default will be determined by *apk* at runtime based
	on how the repository is referenced:
	- ${arch}/${name}-${version}.apk if referenced by repository base path URL
	- ${name}-${version}.apk if referenced by repository index file URL

	Currently supported substitution variables are:
	- name
	- version
	- arch
	- hash

	Additionally a prefix of the variable can used with syntax: *${name:4}*. This
	truncates the substitution to maximum of 4 characters.

	The specification writer should ensure that the repository does not contain
	multiple packages that would expand to same package filename.
