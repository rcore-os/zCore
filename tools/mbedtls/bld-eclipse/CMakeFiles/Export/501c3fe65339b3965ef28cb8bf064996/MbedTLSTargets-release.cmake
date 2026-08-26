#----------------------------------------------------------------
# Generated CMake target import file for configuration "Release".
#----------------------------------------------------------------

# Commands may need to know the format version.
set(CMAKE_IMPORT_FILE_VERSION 1)

# Import target "MbedTLS::everest" for configuration "Release"
set_property(TARGET MbedTLS::everest APPEND PROPERTY IMPORTED_CONFIGURATIONS RELEASE)
set_target_properties(MbedTLS::everest PROPERTIES
  IMPORTED_LINK_INTERFACE_LANGUAGES_RELEASE "C"
  IMPORTED_LOCATION_RELEASE "${_IMPORT_PREFIX}/lib/libeverest.a"
  )

list(APPEND _cmake_import_check_targets MbedTLS::everest )
list(APPEND _cmake_import_check_files_for_MbedTLS::everest "${_IMPORT_PREFIX}/lib/libeverest.a" )

# Import target "MbedTLS::p256m" for configuration "Release"
set_property(TARGET MbedTLS::p256m APPEND PROPERTY IMPORTED_CONFIGURATIONS RELEASE)
set_target_properties(MbedTLS::p256m PROPERTIES
  IMPORTED_LINK_INTERFACE_LANGUAGES_RELEASE "C"
  IMPORTED_LOCATION_RELEASE "${_IMPORT_PREFIX}/lib/libp256m.a"
  )

list(APPEND _cmake_import_check_targets MbedTLS::p256m )
list(APPEND _cmake_import_check_files_for_MbedTLS::p256m "${_IMPORT_PREFIX}/lib/libp256m.a" )

# Import target "MbedTLS::mbedcrypto" for configuration "Release"
set_property(TARGET MbedTLS::mbedcrypto APPEND PROPERTY IMPORTED_CONFIGURATIONS RELEASE)
set_target_properties(MbedTLS::mbedcrypto PROPERTIES
  IMPORTED_LINK_INTERFACE_LANGUAGES_RELEASE "C"
  IMPORTED_LOCATION_RELEASE "${_IMPORT_PREFIX}/lib/libmbedcrypto.a"
  )

list(APPEND _cmake_import_check_targets MbedTLS::mbedcrypto )
list(APPEND _cmake_import_check_files_for_MbedTLS::mbedcrypto "${_IMPORT_PREFIX}/lib/libmbedcrypto.a" )

# Import target "MbedTLS::mbedx509" for configuration "Release"
set_property(TARGET MbedTLS::mbedx509 APPEND PROPERTY IMPORTED_CONFIGURATIONS RELEASE)
set_target_properties(MbedTLS::mbedx509 PROPERTIES
  IMPORTED_LINK_INTERFACE_LANGUAGES_RELEASE "C"
  IMPORTED_LOCATION_RELEASE "${_IMPORT_PREFIX}/lib/libmbedx509.a"
  )

list(APPEND _cmake_import_check_targets MbedTLS::mbedx509 )
list(APPEND _cmake_import_check_files_for_MbedTLS::mbedx509 "${_IMPORT_PREFIX}/lib/libmbedx509.a" )

# Import target "MbedTLS::mbedtls" for configuration "Release"
set_property(TARGET MbedTLS::mbedtls APPEND PROPERTY IMPORTED_CONFIGURATIONS RELEASE)
set_target_properties(MbedTLS::mbedtls PROPERTIES
  IMPORTED_LINK_INTERFACE_LANGUAGES_RELEASE "C"
  IMPORTED_LOCATION_RELEASE "${_IMPORT_PREFIX}/lib/libmbedtls.a"
  )

list(APPEND _cmake_import_check_targets MbedTLS::mbedtls )
list(APPEND _cmake_import_check_files_for_MbedTLS::mbedtls "${_IMPORT_PREFIX}/lib/libmbedtls.a" )

# Commands beyond this point should not need to know the version.
set(CMAKE_IMPORT_FILE_VERSION)
