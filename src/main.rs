#![no_std]
#![no_main]

use core::arch::global_asm;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, Ordering};

mod console;
mod fat32;
mod gic;
mod virtio_blk;
mod virtio_gpu;

global_asm!(include_str!("boot.s"));
global_asm!(include_str!("vectors.s"));

#[unsafe(no_mangle)]
pub extern "C" fn kernel_main() -> ! {
    if let Err(error) = console::init() {
        panic!("virtio-gpu initialization failed: {error}");
    }

    if let Err(error) = gic::init() {
        panic!("GICv3 initialization failed: {error}");
    }

    match fat32::Fat32::mount() {
        Ok(volume) => {
            println!("FAT32 mounted: {} sectors", volume.total_sectors())
                .unwrap_or_else(|_| panic!("virtio-gpu output failed"));
            volume
                .list_root(|entry| {
                    println!(
                        "{}{} {} bytes",
                        if entry.is_directory() { "DIR " } else { "FILE" },
                        entry.name(),
                        entry.size()
                    )
                    .unwrap_or_else(|_| panic!("virtio-gpu output failed"));
                    true
                })
                .unwrap_or_else(|error| panic!("FAT32 root directory read failed: {error}"));

            let mut file_contents = [0; 128];
            match volume.read_file_83("HELLO.TXT", &mut file_contents) {
                Ok(length) => {
                    println!("HELLO.TXT:").unwrap_or_else(|_| panic!("virtio-gpu output failed"));
                    match core::str::from_utf8(&file_contents[..length]) {
                        Ok(contents) => println!("{}", contents)
                            .unwrap_or_else(|_| panic!("virtio-gpu output failed")),
                        Err(_) => println!("(file is not UTF-8 text)")
                            .unwrap_or_else(|_| panic!("virtio-gpu output failed")),
                    }
                }
                Err(fat32::Error::NotFound) => {
                    println!("HELLO.TXT not present")
                        .unwrap_or_else(|_| panic!("virtio-gpu output failed"));
                }
                Err(error) => panic!("HELLO.TXT read failed: {error}"),
            }
        }
        Err(error) => println!("Filesystem unavailable: {}", error)
            .unwrap_or_else(|_| panic!("virtio-gpu output failed")),
    }

    println!("MoveKernel ARM").unwrap_or_else(|_| panic!("virtio-gpu output failed"));
    println!("GICv3 initialized; generic timer IRQ enabled")
        .unwrap_or_else(|_| panic!("virtio-gpu output failed"));
    println!("virtio-gpu console ready").unwrap_or_else(|_| panic!("virtio-gpu output failed"));

    let mut last_tick = gic::ticks();

    loop {
        unsafe {
            core::arch::asm!("wfe", options(nomem, nostack, preserves_flags));
        }
        let current_tick = gic::ticks();
        if current_tick != last_tick {
            last_tick = current_tick;
            println!("Timer ticks: {}", current_tick)
                .unwrap_or_else(|_| panic!("virtio-gpu output failed"));
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
