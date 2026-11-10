#![no_std]
#![no_main]

use core::arch::global_asm;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, Ordering};

mod console;
mod virtio_gpu;

global_asm!(include_str!("boot.s"));

#[unsafe(no_mangle)]
pub extern "C" fn kernel_main() -> ! {
    if let Err(error) = console::init() {
        panic!("virtio-gpu initialization failed: {error}");
    }

    println!("MoveKernel ARM").unwrap_or_else(|_| panic!("virtio-gpu output failed"));
    println!("QEMU {} console ready", "virtio-gpu")
        .unwrap_or_else(|_| panic!("virtio-gpu output failed"));

    loop {
        unsafe {
            core::arch::asm!("wfe", options(nomem, nostack, preserves_flags));
        }
    }
}

static PANICKING: AtomicBool = AtomicBool::new(false);

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    if !PANICKING.swap(true, Ordering::Relaxed) && console::is_initialized() {
        let _ = console::print(format_args!("KERNEL PANIC: {info}\n"));
    }

    loop {
        unsafe {
            core::arch::asm!("wfe", options(nomem, nostack, preserves_flags));
        }
    }
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {
        $crate::console::print(core::format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! println {
    () => {
        $crate::console::print(core::format_args!("\n"))
    };
    ($fmt:expr $(, $($arg:tt)*)?) => {
        $crate::console::print(core::format_args!(concat!($fmt, "\n") $(, $($arg)*)?))
    };
}
