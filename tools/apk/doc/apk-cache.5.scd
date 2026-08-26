apk-cache(5)

# NAME

*/etc/apk/cache* - local cache of package files

# DESCRIPTION

If */etc/apk/cache* is a symlink to a local directory, *apk*(8) will use it as
a local cache for downloaded package files and repository indices. The cache
must not reside on a tmpfs.

To enable the apk cache, run the following commands as root:

mkdir -p /var/cache/apk++
ln -s /var/cache/apk /etc/apk/cache

For information on cache maintenance, see *apk-cache*(8).
