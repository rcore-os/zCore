#!/usr/bin/env python3
"""Generate ML-DSA test cases.
"""

import sys

import maintainer_scripts_path # pylint: disable=unused-import
from mbedtls_framework import test_data_generation
from mbedtls_maintainer import mldsa_test_generator

class MLDSATestGenerator(test_data_generation.TestGenerator):
    """Generate test cases for ML-DSA."""

    def __init__(self, settings) -> None:
        self.targets = {
            'test_suite_pqcp_mldsa.dilithium_py': mldsa_test_generator.gen_pqcp_mldsa_all,
            'test_suite_psa_crypto_mldsa.dilithium_py': \
            lambda: mldsa_test_generator.DriverGenerator().gen_all(),
            'test_suite_dispatch_transparent.dilithium_py': \
            lambda: mldsa_test_generator.DispatchGenerator().gen_all(),
        }
        super().__init__(settings)


if __name__ == '__main__':
    test_data_generation.main(sys.argv[1:], __doc__, MLDSATestGenerator)
