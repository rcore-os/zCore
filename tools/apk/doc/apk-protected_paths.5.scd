apk-protected_paths(5)

# NAME

*/etc/apk/protected_paths.d/\*.list* - paths with special treatment by apk

# DESCRIPTION

Files in _/etc/apk/protected_paths.d/\*.list_ enumerate files are protected and
are not overwritten by *apk*(8). Generally, these are configuration files that
are expected to be modified by the system administrator. These files also
receive special treatment by *apk-audit*(8).

If *apk*(8) would install a file into a protected path which has been modified,
it shall intend write the file into a file with the _.apk-new_ suffix.

For example, once the file _/etc/passwd_ is modified by the local administrator,
*apk*(8) should not overwrite it when upgrading or fixing packages. Likewise,
protected files modified by any automation (including post-install scripts) are
not overwritten.

*apk-audit(8)* shall report protected paths by default. When using
*apk audit --system* or *apk audit --full*, protected files shall be omitted
from the output. When  using *apk audit --backup*, matching files are always
reported. This is in turn used by *lbu commit*.

# FILE FORMAT

Each line is a single rule composed of one symbol followed with a glob
expression, which shall be evaluated relative to the root directory.

The initial symbol must be one of:

*+*
	Protect matching paths only if the file's checksum does not match the
	one in the apk database.

*-*
	Do not protect matching paths, even if it matched a previous rule.

*@*
	Protect matching paths only if they are symlinks which have been
	modified.

*!*
	Protect matching path unconditionally.

Lines starting with *#* and empty lines are ignored.

# EXAMPLES

```
# This line is ignored; it is a comment.
+etc/
@etc/init.d
!etc/apk
\-etc/ssl/certs/ca-cert-\*.pem
```

# CAVEATS

If a file is modified, and its contents eventually become the same as what was
originally installed by apk, the file is considered to have been unmodified.

Wildcard patterns are not taken into account by *apk*(8) when creating
*.apk-new* files instead of overwriting files.

# SEE ALSO

*apk*(8), *apk-audit*(8)
