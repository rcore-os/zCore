# Install script for directory: /home/moebius/eclipse/userland/mbedtls_src/include

# Set the install prefix
if(NOT DEFINED CMAKE_INSTALL_PREFIX)
  set(CMAKE_INSTALL_PREFIX "/home/moebius/eclipse/eclipse-os-build/usr")
endif()
string(REGEX REPLACE "/$" "" CMAKE_INSTALL_PREFIX "${CMAKE_INSTALL_PREFIX}")

# Set the install configuration name.
if(NOT DEFINED CMAKE_INSTALL_CONFIG_NAME)
  if(BUILD_TYPE)
    string(REGEX REPLACE "^[^A-Za-z0-9_]+" ""
           CMAKE_INSTALL_CONFIG_NAME "${BUILD_TYPE}")
  else()
    set(CMAKE_INSTALL_CONFIG_NAME "Release")
  endif()
  message(STATUS "Install configuration: \"${CMAKE_INSTALL_CONFIG_NAME}\"")
endif()

# Set the component getting installed.
if(NOT CMAKE_INSTALL_COMPONENT)
  if(COMPONENT)
    message(STATUS "Install component: \"${COMPONENT}\"")
    set(CMAKE_INSTALL_COMPONENT "${COMPONENT}")
  else()
    set(CMAKE_INSTALL_COMPONENT)
  endif()
endif()

# Install shared libraries without execute permission?
if(NOT DEFINED CMAKE_INSTALL_SO_NO_EXE)
  set(CMAKE_INSTALL_SO_NO_EXE "1")
endif()

# Is this installation the result of a crosscompile?
if(NOT DEFINED CMAKE_CROSSCOMPILING)
  set(CMAKE_CROSSCOMPILING "TRUE")
endif()

# Set default install directory permissions.
if(NOT DEFINED CMAKE_OBJDUMP)
  set(CMAKE_OBJDUMP "/home/moebius/eclipse/eclipse-os-build/bin/objdump")
endif()

if(CMAKE_INSTALL_COMPONENT STREQUAL "Unspecified" OR NOT CMAKE_INSTALL_COMPONENT)
  file(INSTALL DESTINATION "${CMAKE_INSTALL_PREFIX}/include/mbedtls" TYPE FILE PERMISSIONS OWNER_READ OWNER_WRITE GROUP_READ WORLD_READ FILES
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/aes.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/aria.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/asn1.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/asn1write.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/base64.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/bignum.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/block_cipher.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/build_info.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/camellia.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/ccm.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/chacha20.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/chachapoly.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/check_config.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/cipher.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/cmac.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/compat-2.x.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/config_adjust_legacy_crypto.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/config_adjust_legacy_from_psa.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/config_adjust_psa_from_legacy.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/config_adjust_psa_superset_legacy.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/config_adjust_ssl.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/config_adjust_x509.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/config_psa.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/constant_time.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/ctr_drbg.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/debug.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/des.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/dhm.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/ecdh.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/ecdsa.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/ecjpake.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/ecp.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/entropy.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/error.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/gcm.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/hkdf.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/hmac_drbg.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/lms.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/mbedtls_config.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/md.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/md5.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/memory_buffer_alloc.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/net_sockets.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/nist_kw.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/oid.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/pem.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/pk.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/pkcs12.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/pkcs5.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/pkcs7.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/platform.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/platform_time.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/platform_util.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/poly1305.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/private_access.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/psa_util.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/ripemd160.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/rsa.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/sha1.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/sha256.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/sha3.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/sha512.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/ssl.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/ssl_cache.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/ssl_ciphersuites.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/ssl_cookie.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/ssl_ticket.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/threading.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/timing.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/version.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/x509.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/x509_crl.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/x509_crt.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/mbedtls/x509_csr.h"
    )
endif()

if(CMAKE_INSTALL_COMPONENT STREQUAL "Unspecified" OR NOT CMAKE_INSTALL_COMPONENT)
  file(INSTALL DESTINATION "${CMAKE_INSTALL_PREFIX}/include/psa" TYPE FILE PERMISSIONS OWNER_READ OWNER_WRITE GROUP_READ WORLD_READ FILES
    "/home/moebius/eclipse/userland/mbedtls_src/include/psa/build_info.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/psa/crypto.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/psa/crypto_adjust_auto_enabled.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/psa/crypto_adjust_config_dependencies.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/psa/crypto_adjust_config_key_pair_types.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/psa/crypto_adjust_config_synonyms.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/psa/crypto_builtin_composites.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/psa/crypto_builtin_key_derivation.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/psa/crypto_builtin_primitives.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/psa/crypto_compat.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/psa/crypto_config.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/psa/crypto_driver_common.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/psa/crypto_driver_contexts_composites.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/psa/crypto_driver_contexts_key_derivation.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/psa/crypto_driver_contexts_primitives.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/psa/crypto_extra.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/psa/crypto_legacy.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/psa/crypto_platform.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/psa/crypto_se_driver.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/psa/crypto_sizes.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/psa/crypto_struct.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/psa/crypto_types.h"
    "/home/moebius/eclipse/userland/mbedtls_src/include/psa/crypto_values.h"
    )
endif()

