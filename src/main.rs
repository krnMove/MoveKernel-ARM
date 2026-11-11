#![no_std]
#![no_main]

extern crate alloc;

use core::arch::global_asm;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, Ordering};

mod console;
mod fat32;
mod gic;
mod heap;
mod mmu;
mod scheduler;
mod shell;
mod virtio_blk;
mod virtio_gpu;
mod virtio_keyboard;

global_asm!(include_str!("boot.s"));
global_asm!(include_str!("vectors.s"));
global_asm!(include_str!("context.s"));

#[unsafe(no_mangle)]
pub extern "C" fn kernel_main() -> ! {
    if let Err(error) = mmu::init() {
        panic!("ARM MMU initialization failed: {error}");
    }
    if let Err(error) = heap::init() {
        panic!("heap initialization failed: {error}");
    }

    if let Err(error) = console::init() {
        panic!("virtio-gpu initialization failed: {error}");
    }

    if let Err(error) = gic::init() {
        panic!("GICv3 initialization failed: {error}");
    }

    if !mmu::is_enabled() {
        panic!("ARM MMU is not enabled");
    }
    if !heap::self_test() {
        panic!("heap allocation/alignment/reuse self-test failed");
    }
    let (volume, disk_sectors, filesystem_error) = match fat32::Fat32::mount() {
        Ok(volume) => (Some(volume), Some(volume.total_sectors()), None),
        Err(error) => (None, None, Some(error)),
    };

    scheduler::init().unwrap_or_else(|error| panic!("scheduler initialization failed: {error}"));
    virtio_keyboard::init()
        .unwrap_or_else(|error| panic!("virtio keyboard initialization failed: {error}"));

    console::draw_startup(
        env!("CARGO_PKG_VERSION"),
        heap::capacity_bytes() / 1024,
        disk_sectors,
        filesystem_error
            .as_ref()
            .map(|error| error as &dyn core::fmt::Display),
    )
    .unwrap_or_else(|_| panic!("virtio-gpu output failed"));

    shell::run(volume)
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
