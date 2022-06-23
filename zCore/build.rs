use std::{env, fs::File, io::Write};

fn main() {
    if env::var("TARGET").unwrap().contains("riscv64") {
        let kernel_base_addr = if env::var("PLATFORM").map_or(false, |p| p.contains("d1")) {
            0xffffffffc0100000usize
        } else {
            0xffffffff80200000usize
        };

        File::create("src/platform/riscv/linker.ld")
            .unwrap()
            .write_all(
                format!(
                    "\
OUTPUT_ARCH(riscv)
ENTRY(_start)
BASE_ADDRESS = {kernel_base_addr:#x};
{RISCV64_SECTIONS}"
                )
                .as_bytes(),
            )
            .unwrap();
    } else if env::var("TARGET").unwrap().contains("aarch64") {
        println!("cargo:rustc-env=USER_IMG=zCore/aarch64.img");
    }
}

const RISCV64_SECTIONS: &str = "
SECTIONS
{
    . = BASE_ADDRESS;
    start = .;

    .text : {
        stext = .;
        *(.text.entry)
        *(.text .text.*)
        etext = .;
    }

    .rodata ALIGN(4K) : {
        srodata = .;
        *(.rodata .rodata.*)
        *(.srodata .srodata.*)
        erodata = .;
    }

    .data ALIGN(4K) : {
        sdata = .;
        *(.data .data.*)
        *(.sdata .sdata.*)
        edata = .;
    }

    .bss ALIGN(4K) : {
        bootstack = .;
        *(.bss.bootstack)
        bootstacktop = .;

        . = ALIGN(4K);
        sbss = .;
        *(.bss .bss.*)
        *(.sbss .sbss.*)
        ebss = .;
    }

    PROVIDE(end = .);
}
";
