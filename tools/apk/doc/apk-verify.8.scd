apk-verify(8)

# NAME

apk verify - verify package integrity and signature

# SYNOPSIS

*apk verify* [<_options_>...] _files_...

# DESCRIPTION

*apk verify* verifies the integrity of the package files specified, validating
the checksums and cryptographic signature.

*apk verify* prints a summary of the results on stdout, and exits with status
code zero if all packages were successfully verified. If any failed, *apk
verify* exits with a non-zero status.

# OPTIONS

*apk verify* does not support any specific options. See *apk*(8) for global
options.
