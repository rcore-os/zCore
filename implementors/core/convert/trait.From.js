(function() {var implementors = {};
implementors["kernel_hal"] = [{"text":"impl <a class=\"trait\" href=\"https://doc.rust-lang.org/nightly/core/convert/trait.From.html\" title=\"trait core::convert::From\">From</a>&lt;<a class=\"enum\" href=\"kernel_hal/defs/enum.CachePolicy.html\" title=\"enum kernel_hal::defs::CachePolicy\">CachePolicy</a>&gt; for <a class=\"primitive\" href=\"https://doc.rust-lang.org/nightly/std/primitive.u32.html\">u32</a>","synthetic":false,"types":[]},{"text":"impl&lt;T, P:&nbsp;<a class=\"trait\" href=\"kernel_hal/user/trait.Policy.html\" title=\"trait kernel_hal::user::Policy\">Policy</a>&gt; <a class=\"trait\" href=\"https://doc.rust-lang.org/nightly/core/convert/trait.From.html\" title=\"trait core::convert::From\">From</a>&lt;<a class=\"primitive\" href=\"https://doc.rust-lang.org/nightly/std/primitive.usize.html\">usize</a>&gt; for <a class=\"struct\" href=\"kernel_hal/user/struct.UserPtr.html\" title=\"struct kernel_hal::user::UserPtr\">UserPtr</a>&lt;T, P&gt;","synthetic":false,"types":["kernel_hal::user::UserPtr"]}];
implementors["linux_object"] = [{"text":"impl <a class=\"trait\" href=\"https://doc.rust-lang.org/nightly/core/convert/trait.From.html\" title=\"trait core::convert::From\">From</a>&lt;<a class=\"enum\" href=\"zircon_object/error/enum.ZxError.html\" title=\"enum zircon_object::error::ZxError\">ZxError</a>&gt; for <a class=\"enum\" href=\"linux_object/error/enum.LxError.html\" title=\"enum linux_object::error::LxError\">LxError</a>","synthetic":false,"types":["linux_object::error::LxError"]},{"text":"impl <a class=\"trait\" href=\"https://doc.rust-lang.org/nightly/core/convert/trait.From.html\" title=\"trait core::convert::From\">From</a>&lt;<a class=\"enum\" href=\"linux_object/fs/vfs/enum.FsError.html\" title=\"enum linux_object::fs::vfs::FsError\">FsError</a>&gt; for <a class=\"enum\" href=\"linux_object/error/enum.LxError.html\" title=\"enum linux_object::error::LxError\">LxError</a>","synthetic":false,"types":["linux_object::error::LxError"]},{"text":"impl <a class=\"trait\" href=\"https://doc.rust-lang.org/nightly/core/convert/trait.From.html\" title=\"trait core::convert::From\">From</a>&lt;<a class=\"enum\" href=\"kernel_hal/user/enum.Error.html\" title=\"enum kernel_hal::user::Error\">Error</a>&gt; for <a class=\"enum\" href=\"linux_object/error/enum.LxError.html\" title=\"enum linux_object::error::LxError\">LxError</a>","synthetic":false,"types":["linux_object::error::LxError"]},{"text":"impl <a class=\"trait\" href=\"https://doc.rust-lang.org/nightly/core/convert/trait.From.html\" title=\"trait core::convert::From\">From</a>&lt;<a class=\"primitive\" href=\"https://doc.rust-lang.org/nightly/std/primitive.usize.html\">usize</a>&gt; for <a class=\"struct\" href=\"linux_object/fs/struct.FileDesc.html\" title=\"struct linux_object::fs::FileDesc\">FileDesc</a>","synthetic":false,"types":["linux_object::fs::FileDesc"]},{"text":"impl <a class=\"trait\" href=\"https://doc.rust-lang.org/nightly/core/convert/trait.From.html\" title=\"trait core::convert::From\">From</a>&lt;<a class=\"primitive\" href=\"https://doc.rust-lang.org/nightly/std/primitive.i32.html\">i32</a>&gt; for <a class=\"struct\" href=\"linux_object/fs/struct.FileDesc.html\" title=\"struct linux_object::fs::FileDesc\">FileDesc</a>","synthetic":false,"types":["linux_object::fs::FileDesc"]},{"text":"impl <a class=\"trait\" href=\"https://doc.rust-lang.org/nightly/core/convert/trait.From.html\" title=\"trait core::convert::From\">From</a>&lt;<a class=\"struct\" href=\"linux_object/fs/struct.FileDesc.html\" title=\"struct linux_object::fs::FileDesc\">FileDesc</a>&gt; for <a class=\"primitive\" href=\"https://doc.rust-lang.org/nightly/std/primitive.usize.html\">usize</a>","synthetic":false,"types":[]},{"text":"impl <a class=\"trait\" href=\"https://doc.rust-lang.org/nightly/core/convert/trait.From.html\" title=\"trait core::convert::From\">From</a>&lt;<a class=\"struct\" href=\"linux_object/fs/struct.FileDesc.html\" title=\"struct linux_object::fs::FileDesc\">FileDesc</a>&gt; for <a class=\"primitive\" href=\"https://doc.rust-lang.org/nightly/std/primitive.i32.html\">i32</a>","synthetic":false,"types":[]},{"text":"impl <a class=\"trait\" href=\"https://doc.rust-lang.org/nightly/core/convert/trait.From.html\" title=\"trait core::convert::From\">From</a>&lt;<a class=\"enum\" href=\"linux_object/signal/enum.Signal.html\" title=\"enum linux_object::signal::Signal\">Signal</a>&gt; for <a class=\"primitive\" href=\"https://doc.rust-lang.org/nightly/std/primitive.u8.html\">u8</a>","synthetic":false,"types":[]},{"text":"impl <a class=\"trait\" href=\"https://doc.rust-lang.org/nightly/core/convert/trait.From.html\" title=\"trait core::convert::From\">From</a>&lt;<a class=\"struct\" href=\"linux_object/time/struct.TimeSpec.html\" title=\"struct linux_object::time::TimeSpec\">TimeSpec</a>&gt; for <a class=\"struct\" href=\"linux_object/fs/vfs/struct.Timespec.html\" title=\"struct linux_object::fs::vfs::Timespec\">Timespec</a>","synthetic":false,"types":["rcore_fs::vfs::Timespec"]},{"text":"impl <a class=\"trait\" href=\"https://doc.rust-lang.org/nightly/core/convert/trait.From.html\" title=\"trait core::convert::From\">From</a>&lt;<a class=\"struct\" href=\"linux_object/time/struct.TimeSpec.html\" title=\"struct linux_object::time::TimeSpec\">TimeSpec</a>&gt; for <a class=\"struct\" href=\"https://doc.rust-lang.org/nightly/core/time/struct.Duration.html\" title=\"struct core::time::Duration\">Duration</a>","synthetic":false,"types":["core::time::Duration"]},{"text":"impl <a class=\"trait\" href=\"https://doc.rust-lang.org/nightly/core/convert/trait.From.html\" title=\"trait core::convert::From\">From</a>&lt;<a class=\"struct\" href=\"linux_object/time/struct.TimeSpec.html\" title=\"struct linux_object::time::TimeSpec\">TimeSpec</a>&gt; for <a class=\"struct\" href=\"linux_object/time/struct.TimeVal.html\" title=\"struct linux_object::time::TimeVal\">TimeVal</a>","synthetic":false,"types":["linux_object::time::TimeVal"]}];
implementors["zircon_object"] = [{"text":"impl <a class=\"trait\" href=\"https://doc.rust-lang.org/nightly/core/convert/trait.From.html\" title=\"trait core::convert::From\">From</a>&lt;<a class=\"enum\" href=\"zircon_object/dev/pci/enum.PcieIrqMode.html\" title=\"enum zircon_object::dev::pci::PcieIrqMode\">PcieIrqMode</a>&gt; for <a class=\"primitive\" href=\"https://doc.rust-lang.org/nightly/std/primitive.u32.html\">u32</a>","synthetic":false,"types":[]},{"text":"impl <a class=\"trait\" href=\"https://doc.rust-lang.org/nightly/core/convert/trait.From.html\" title=\"trait core::convert::From\">From</a>&lt;<a class=\"enum\" href=\"zircon_object/dev/enum.ResourceKind.html\" title=\"enum zircon_object::dev::ResourceKind\">ResourceKind</a>&gt; for <a class=\"primitive\" href=\"https://doc.rust-lang.org/nightly/std/primitive.u32.html\">u32</a>","synthetic":false,"types":[]},{"text":"impl <a class=\"trait\" href=\"https://doc.rust-lang.org/nightly/core/convert/trait.From.html\" title=\"trait core::convert::From\">From</a>&lt;<a class=\"enum\" href=\"kernel_hal/user/enum.Error.html\" title=\"enum kernel_hal::user::Error\">Error</a>&gt; for <a class=\"enum\" href=\"zircon_object/enum.ZxError.html\" title=\"enum zircon_object::ZxError\">ZxError</a>","synthetic":false,"types":["zircon_object::error::ZxError"]},{"text":"impl <a class=\"trait\" href=\"https://doc.rust-lang.org/nightly/core/convert/trait.From.html\" title=\"trait core::convert::From\">From</a>&lt;<a class=\"struct\" href=\"zircon_object/signal/struct.PortPacketRepr.html\" title=\"struct zircon_object::signal::PortPacketRepr\">PortPacketRepr</a>&gt; for <a class=\"struct\" href=\"zircon_object/signal/struct.PortPacket.html\" title=\"struct zircon_object::signal::PortPacket\">PortPacket</a>","synthetic":false,"types":["zircon_object::signal::port::port_packet::PortPacket"]},{"text":"impl <a class=\"trait\" href=\"https://doc.rust-lang.org/nightly/core/convert/trait.From.html\" title=\"trait core::convert::From\">From</a>&lt;&amp;'_ <a class=\"struct\" href=\"zircon_object/signal/struct.PortPacket.html\" title=\"struct zircon_object::signal::PortPacket\">PortPacket</a>&gt; for <a class=\"struct\" href=\"zircon_object/signal/struct.PortPacketRepr.html\" title=\"struct zircon_object::signal::PortPacketRepr\">PortPacketRepr</a>","synthetic":false,"types":["zircon_object::signal::port::port_packet::PortPacketRepr"]},{"text":"impl <a class=\"trait\" href=\"https://doc.rust-lang.org/nightly/core/convert/trait.From.html\" title=\"trait core::convert::From\">From</a>&lt;<a class=\"enum\" href=\"zircon_object/task/enum.ThreadStateKind.html\" title=\"enum zircon_object::task::ThreadStateKind\">ThreadStateKind</a>&gt; for <a class=\"primitive\" href=\"https://doc.rust-lang.org/nightly/std/primitive.u32.html\">u32</a>","synthetic":false,"types":[]},{"text":"impl <a class=\"trait\" href=\"https://doc.rust-lang.org/nightly/core/convert/trait.From.html\" title=\"trait core::convert::From\">From</a>&lt;<a class=\"enum\" href=\"zircon_object/vm/enum.SeekOrigin.html\" title=\"enum zircon_object::vm::SeekOrigin\">SeekOrigin</a>&gt; for <a class=\"primitive\" href=\"https://doc.rust-lang.org/nightly/std/primitive.usize.html\">usize</a>","synthetic":false,"types":[]}];
if (window.register_implementors) {window.register_implementors(implementors);} else {window.pending_implementors = implementors;}})()