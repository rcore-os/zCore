var N = null;var sourcesIndex = {};
sourcesIndex["kernel_hal_bare"] = {"name":"","dirs":[{"name":"arch","files":["mod.rs","x86_64.rs"]}],"files":["lib.rs"]};
sourcesIndex["kernel_hal_unix"] = {"name":"","files":["fsbase_macos.rs","lib.rs"]};
sourcesIndex["linux_loader"] = {"name":"","files":["abi.rs","lib.rs"]};
sourcesIndex["linux_syscall"] = {"name":"","dirs":[{"name":"fs","files":["device.rs","file.rs","ioctl.rs","mod.rs","pseudo.rs","random.rs","stdio.rs"]},{"name":"syscall","dirs":[{"name":"file","files":["dir.rs","fd.rs","file.rs","mod.rs","poll.rs","stat.rs"]}],"files":["consts.rs","misc.rs","mod.rs","task.rs","vm.rs"]}],"files":["error.rs","lib.rs","process.rs","util.rs"]};
sourcesIndex["zircon_loader"] = {"name":"","files":["lib.rs","vdso.rs"]};
sourcesIndex["zircon_object"] = {"name":"","dirs":[{"name":"io","files":["event.rs","mod.rs","port.rs","timer.rs"]},{"name":"ipc","files":["channel.rs","fifo.rs","mod.rs","socket.rs"]},{"name":"object","files":["handle.rs","mod.rs","rights.rs","signal.rs"]},{"name":"task","files":["exception.rs","job.rs","job_policy.rs","mod.rs","process.rs","thread.rs"]},{"name":"util","files":["block_range.rs","mod.rs"]},{"name":"vm","dirs":[{"name":"vmo","files":["mod.rs","paged.rs","physical.rs"]}],"files":["mod.rs","vmar.rs"]}],"files":["error.rs","hal.rs","lib.rs","resource.rs"]};
sourcesIndex["zircon_syscall"] = {"name":"","files":["channel.rs","consts.rs","debug.rs","debuglog.rs","handle.rs","lib.rs","task.rs","util.rs","vmar.rs","vmo.rs"]};
createSourceSidebar();
