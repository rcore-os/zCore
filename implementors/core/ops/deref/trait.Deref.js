(function() {var implementors = {};
implementors["kernel_hal"] = [{"text":"impl&lt;P:&nbsp;Policy&gt; Deref for IoVecs&lt;P&gt;","synthetic":false,"types":[]}];
implementors["linux_object"] = [{"text":"impl Deref for STDIN","synthetic":false,"types":[]},{"text":"impl Deref for STDOUT","synthetic":false,"types":[]},{"text":"impl&lt;'a&gt; Deref for SemaphoreGuard&lt;'a&gt;","synthetic":false,"types":[]}];
implementors["zircon_object"] = [{"text":"impl Deref for CurrentThread","synthetic":false,"types":[]},{"text":"impl Deref for VmObject","synthetic":false,"types":[]},{"text":"impl Deref for KERNEL_ASPACE","synthetic":false,"types":[]}];
if (window.register_implementors) {window.register_implementors(implementors);} else {window.pending_implementors = implementors;}})()