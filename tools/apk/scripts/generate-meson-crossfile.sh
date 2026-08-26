#!/bin/sh

set -eu

_target_endianess=little
_target_cpu="$CARCH"

case "$CARCH" in
	mips*)
		_target_endianness=big
		_target_cpu_family=mips
		;;
	arm*)
		_target_cpu_family=arm
		;;
	ppc64le)
		_target_cpu_family=ppc64
		;;
	aarch64|x86*)
		# $CARCH maps 1:1 to _cpu_family for meson for these arches
		_target_cpu_family="$CARCH"
		;;
esac

# Keep in mind that CC, CXX etc. are the binaries to compile from host
# to target, not from host to host!
cat > apk.cross <<EOF
[binaries]
c = '${CC}'
cpp = '${CXX}'
ar = '${AR}'
nm = '${NM}'
ld = '${LD}'
strip = '${STRIP}'
readelf = '${READELF}'
objcopy = '${OBJCOPY}'
pkgconfig = 'pkg-config'
[properties]
needs_exe_wrapper = true
c_args = ['$(echo ${CFLAGS} | sed -r "s/\s+/','/g")']
c_link_args = ['$(echo ${LDFLAGS} | sed -r "s/\s+/','/g")']
cpp_args = ['$(echo ${CXXFLAGS} | sed -r "s/\s+/','/g")']
cpp_link_args = ['$(echo ${LDFLAGS} | sed -r "s/\s+/','/g")']
[host_machine]
system = 'linux'
cpu_family = '${_target_cpu_family}'
cpu = '${_target_cpu}'
endian = '${_target_endianess}'
EOF

echo "Generating crossfile is done. You can invoke meson with the cross file with 'meson --cross apk.cross' now."
