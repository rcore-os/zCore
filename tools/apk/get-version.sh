#!/bin/sh

try_version() {
	# usable version?
	[ "${#1}" -gt 0 ] || return 0
	# strip the git tag prefix
	echo "${1#v}"
	exit 0
}

# check for build system provided forced version
for version in "$@"; do
	try_version "$version"
done
try_version "${VERSION}"
try_version "${CI_COMMIT_TAG}"
# GitLab but no tag info, use the 'git describe' from environment variable
# once https://gitlab.com/gitlab-org/gitlab-runner/-/merge_requests/1633
# gets completed and merged upstream.
[ -n "$CI_COMMIT_REF_NAME" ] && try_version "$(cat VERSION)"
[ -d .git ] && try_version "$(git describe)"
try_version "$(cat VERSION)"
exit 1
