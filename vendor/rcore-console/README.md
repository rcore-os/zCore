# rcore-console

[![Actions Status](https://github.com/rcore-os/rcore-console/workflows/CI/badge.svg)](https://github.com/rcore-os/rcore-console/actions)

The virtual console embedded in rCore kernel.

## Run example

1. rterm

Read and show stdin:

```
htop | cargo run --example rterm
```

2. pty

Spawn a process and show:

```
cargo run --example pty htop
```

TODO: documents and tests
