.section .text._start
.global _start
_start:
    // Set up stack (use end of RAM as stack top)
    ldr x0, =0x48000000
    mov sp, x0

    // Rust may emit FP/SIMD instructions even when the kernel does not use them explicitly.
    mrs x0, cpacr_el1
    mov x1, #0x300000
    orr x0, x0, x1
    msr cpacr_el1, x0
    isb

    // Rust statics are stored in .bss and must be initialized before use.
    ldr x0, =__bss_start
    ldr x1, =__bss_end
1:
    cmp x0, x1
    b.hs 2f
    str xzr, [x0], #8
    b 1b
2:
    // Jump to Rust
    bl kernel_main

    // If kernel_main returns (it shouldn't), loop forever
3:  b 3b