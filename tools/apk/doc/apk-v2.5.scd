apk-v2(5)

# NAME

apk v2 - overview of apk v2 format

# DESCRIPTION

A v2 .apk file contains a single package's contents, some metadata, and
some signatures. The .apk file contains three concatenated gzip streams,
which together form a single tar archive. The tar archive contains three
sections: the signatures, the control section, and the data section.

# THE SIGNATURES

The signatures are a sequence of files whose names start with ".SIGN.",
which must come before any other data in the tarball. These filenames
look like:

	*.SIGN.<algorithm>.<keyid>*

where <algorithm> must be one of *DSA*, *RSA*, *RSA256*, and *RSA512*
and <keyid> must be the name of the key's file in /etc/apk/keys (see
*apk-keys*(5)).

The signature can be computed over either the metadata (if the metadata
contains a data hash for the data), or over the metadata and data
together (if the metadata contains no data hash).

A single signature from a trusted key is sufficient, so an apk can be
signed by multiple different keys if need be, as long as clients trust
at least one of them.

# THE CONTROL SECTION

In a v2 apk file, the package metadata is stored in a single file called
.PKGINFO. That file uses a key-value format, in which keys and values
are separated by " = " and lines beginning with "#" are comments. There
are many allowed keys and there is no centralized list of known keys;
the source of *abuild*(1) is the best reference.

One key is important for understanding the v2 format because it affects
the interpretation of the signature: if there is a "datahash" key in
PKGINFO, its value is the sha256 hash of the data part of the apk.
Packages are supposed to have a datahash, but indexes do not.

The control section is also where pre/post hook scripts for install, deinstall,
and upgrade live, and where triggers live.

# THE DATA SECTION

The data section is simply a tar archive of the package's contents, as
produced by the build process. These files are postprocessed by
*abuild-tar*(1) and use pax extended headers to include per-file
checksums in a header named APK-TOOLS.checksum.*<hash>*.

# EXAMPLE

As an example, the v2 apk for *scdoc*(1) itself contains these files in
this order:

```
.SIGN.RSA.alpine-devel@lists.alpinelinux.org-6165ee59.rsa.pub
.PKGINFO
usr/
usr/bin/
usr/bin/scdoc
usr/share/
usr/share/pkgconfig/
usr/share/pkgconfig/scdoc.pc
```

Since v2 apk files are simply tarballs (broken into multiple gzip
streams), they can be inspected and unpacked with *tar*(1), although
care must be taken when changing them not to reorder the sections or
invalidate the signature. It is better to use *abuild*(1) to modify
them. If you want to take them apart into their constituent gzip
streams, you can use *abuild-gzsplit*(1).

# NOTES

Only the "RSA" (meaning RSA + SHA1) signature scheme is currently used
by *abuild*(1).

# SEE ALSO

*abuild*(1), *apk*(8), *apk-package*(5), *apk-v3*(5)
