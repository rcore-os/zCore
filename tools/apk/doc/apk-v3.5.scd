apk-v3(5)

# NAME

apk v3 - overview of apk v3 format

# DESCRIPTION

A v3 .apk file contains a single package's contents, some metadata, and
some signatures. The .apk file contains a tree of objects, represented
in a custom binary format and conforming overall to a pre-defined
schema. This file format is referred to inside *apk*(5) as "adb".

# WIRE FORMAT

A v3 apk file is composed of sequences of serialized values, each of
which begins with a 32-bit little-endian word - the value's tag. The
high 4 bits of the tag are a type code, and the low 28 bits are used for
an immediate value. Defined type codes are:

|[ 0x0
:[ Special
:[ (direct)
|  0x1
:  Int
:  (direct)
|  0x2
:  Int32
:  (indirect)
|  0x3
:  Int64
:  (indirect)
|  0x8
:  Blob8
:  (indirect)
|  0x9
:  Blob16
:  (indirect)
|  0xa
:  Blob32
:  (indirect)
|  0xd
:  Array
:  (indirect)
|  0xe
:  Object
:  (indirect)

A direct value is packed into the low 28 bits of the tag word; an
indirect value is instead stored elsewhere in the file, and the offset
of that indirect value is packed into the low 28 bits of the tag word.

Arrays and objects are represented with a sequence of numbered slots;
the value packed into their tag word is the offset at which this
sequence starts. The first slot is always the total number of slots, so
all arrays and objects contain at least one item.

The only real difference between arrays and objects in the wire encoding
is that arrays are homogenous, whereas objects are heterogeneous with a
separate defined type for each slot.

The special type is used to represent three atoms:

|[ 0x0
:[ NULL
|  0x1
:  TRUE
|  0x2
:  FALSE

# FILE SCHEMAS

A schema is a representation of what data elements are expected in an
adb file. Schemas form a tree, where nodes are either scalar schemas
(which are leaves in the tree) or array/object schemas, which themselves
have children. For example, the schema for a package object might
declare that it contains fields which themselves conform to the string
array schema, or the pkginfo schema, or similar.

The schemas themselves are not represented in the adb file in any way;
they exist in the parts of *apk*(1) that read and write such files. A
full description of all of apk's schemas would be lengthy, but as an
example, here is the schema for a single file inside a package:

|[ ADBI_FI_NAME
:[ "name"
:[ string
|  ADBI_FI_ACL
:  "acl"
:  acl
|  ADBI_FI_SIZE
:  "size"
:  int
|  ADBI_FI_MTIME
:  "mtime"
:  int
|  ADBI_FI_HASHES
:  "hash"
:  hexblob
|  ADBI_FI_TARGET
:  "target"
:  hexblob

Here, all of the fields except for "acl" are scalars, and acl is itself
a schema looking like:

|[ ADBI_ACL_MODE
:[ "mode"
:[ oct
|  ADBI_ACL_USER
:  "user"
:  string
|  ADBI_ACL_GROUP
:  "group"
:  string

# BLOCKS

An actual adb file is composed of a sequence of typed blocks; a block
also begins with a 32-bit little-endian tag word, which has two bits of
type and 30 bits of size. The two type bits are:

|[ 0x0
:[ ADB
|  0x1
:  SIG
|  0x2
:  DATA
|  0x3
:  DATAX

The adb file must begin with one ADB block, then optionally one or more
SIG blocks, then one or more DATA blocks. The ADB block must begin with a
magic number indicating the schema for the entire ADB block's root
object. The ADB block also contains, outside the root object, some
metadata describing the version of the adb format in use.

The SIG block contains one or more signatures of the ADB block. Signatures
of the same version should be in the same SIG block. If in future, a new
signature version is specified, and package should contain for compatibility
reasons two different versions of signature during transition time, then there
should be two signature blocks, one of each version.

Unlike the v2 format, the key name used for the signature is not
explicitly specified. Instead an intrinsic ID of the key is used for the
lookup, so verifiers must find the key based in the key ID. Also unlike
the v2 format, the ADB block is not signed directly, but it is hashed
first by a secure digest (currently SHA512). After this a small payload
with this pre-calculated digest is signed by given algorithm (usually
the payload is then hashed again by the signing process with a secure
digest based on the signature algorithm).

The DATA blocks are used to store package file data only; all file
metadata, including content hashes, is stored in the ADB block instead.
The contents of the DATA blocks are therefore protected by the hashes
given in the ADB block, which is itself protected by the signature in
the SIG block.

It is currently illegal for a DATAX block to appear.

# NOTES

The v3 file format is entangled with C struct layout, since it sometimes
directly writes structs into the adb section, including any
compiler-added padding and such.

# SEE ALSO

*abuild*(1), *apk*(8), *apk-package*(5), *apk-v2*(5)
