apk-index(8)

# NAME

apk index - create repository index file from packages

# SYNOPSIS

*apk index* [<_options_>...] _packages_...

# DESCRIPTION

*apk index* creates a repository index from a list of package files. See
*apk-repositories*(5) for more information on repository indices.

Generally, the resulting index must be cryptographically signed before *apk*
will accept it. See *abuild-sign*(1) for details.

# OPTIONS

*--description*, *-d* _TEXT_
	Add a description to the index. Upstream, this is used to add version
	information based on the git commit SHA of aports HEAD at the time of
	index generation.

*--index*, *-x* _INDEX_
	Read an existing index from _INDEX_ to speed up the creation of the new
	index by reusing data when possible.

*--merge*
	Merge _packages_ into the existing _INDEX_.

*--no-warnings*
	Disable the warning about missing dependencies. This happens when A,
	depends on package B, that does not have a provider in the indexed
	repository.

*--output*, *-o* _FILE_
	Output generated index to _FILE_.

*--prune-origin*
	Prune packages from the existing _INDEX_ with same origin as any of
	the new _packages_ during merge.

*--rewrite-arch* _ARCH_
	Set all package's architecture to _ARCH_.

# ENVIRONMENT

*SOURCE_DATE_EPOCH*
	Used as the build time for tar file headers in APKINDEX.tar.gz.
