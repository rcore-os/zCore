apk-update(8)

# NAME

apk update - update repository indexes

# SYNOPSIS

*apk update*

# DESCRIPTION

*apk update* forces updating of the indexes from all configured package
repositories. This command is not needed in normal operation as all applets
requiring indexes will automatically refresh them after caching time expires.

See *apk-repositories*(5) for more information on configuring package
repositories.

# OPTIONS

*apk update* does not support any specific options. See *apk*(8) for global
options.
