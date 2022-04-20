
#!/bin/bash

cd .. && make baremetal-test-rv64 ROOTPROC=/libc-test/regression/printf-1e9-oob.exe?
