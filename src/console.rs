use core::fmt::{self, Write};
use core::ptr::{addr_of_mut, copy, write_volatile};
use core::sync::atomic::{AtomicBool, Ordering};

use crate::virtio_gpu::{self, Error as GpuError};

const MAX_WIDTH: usize = 1280;
const MAX_HEIGHT: usize = 800;
const CELL_WIDTH: usize = 6;
const CELL_HEIGHT: usize = 8;
const BACKGROUND: u32 = 0xff00_0000;
const FOREGROUND: u32 = 0xffff_ffff;

#[repr(align(4096))]
struct Framebuffer([u32; MAX_WIDTH * MAX_HEIGHT]);

static mut FRAMEBUFFER: Framebuffer = Framebuffer([0; MAX_WIDTH * MAX_HEIGHT]);
static mut WIDTH: usize = 0;
static mut HEIGHT: usize = 0;
static mut CURSOR_X: usize = 8;
static mut CURSOR_Y: usize = 8;
static INITIALIZED: AtomicBool = AtomicBool::new(false);

const FONT: [[u8; 7]; 36] = [
    [14, 17, 17, 31, 17, 17, 17], // A
    [30, 17, 17, 30, 17, 17, 30], // B
    [14, 17, 16, 16, 16, 17, 14], // C
    [30, 17, 17, 17, 17, 17, 30], // D
    [31, 16, 16, 30, 16, 16, 31], // E
    [31, 16, 16, 30, 16, 16, 16], // F
    [14, 17, 16, 23, 17, 17, 15], // G
    [17, 17, 17, 31, 17, 17, 17], // H
    [14, 4, 4, 4, 4, 4, 14],      // I
    [7, 2, 2, 2, 18, 18, 12],     // J
    [17, 18, 20, 24, 20, 18, 17], // K
    [16, 16, 16, 16, 16, 16, 31], // L
    [17, 27, 21, 21, 17, 17, 17], // M
    [17, 25, 21, 19, 17, 17, 17], // N
    [14, 17, 17, 17, 17, 17, 14], // O
    [30, 17, 17, 30, 16, 16, 16], // P
    [14, 17, 17, 17, 21, 18, 13], // Q
    [30, 17, 17, 30, 20, 18, 17], // R
    [15, 16, 16, 14, 1, 1, 30],   // S
    [31, 4, 4, 4, 4, 4, 4],       // T
    [17, 17, 17, 17, 17, 17, 14], // U
    [17, 17, 17, 17, 17, 10, 4],  // V
    [17, 17, 17, 21, 21, 21, 10], // W
    [17, 17, 10, 4, 10, 17, 17],  // X
    [17, 17, 10, 4, 4, 4, 4],     // Y
    [31, 1, 2, 4, 8, 16, 31],     // Z
    [14, 17, 19, 21, 25, 17, 14], // 0
    [4, 12, 4, 4, 4, 4, 14],      // 1
    [14, 17, 1, 2, 4, 8, 31],     // 2
    [30, 1, 1, 14, 1, 1, 30],     // 3
    [2, 6, 10, 18, 31, 2, 2],     // 4
    [31, 16, 16, 30, 1, 1, 30],   // 5
    [14, 16, 16, 30, 17, 17, 14], // 6
    [31, 1, 2, 4, 8, 8, 8],       // 7
    [14, 17, 17, 14, 17, 17, 14], // 8
    [14, 17, 17, 15, 1, 1, 14],   // 9
];

pub fn init() -> Result<(), GpuError> {
    let framebuffer = unsafe { addr_of_mut!(FRAMEBUFFER.0) as *mut u32 };
    let mode = virtio_gpu::initialize(framebuffer, MAX_WIDTH, MAX_HEIGHT)?;
    unsafe {
        WIDTH = mode.width;
        HEIGHT = mode.height;
        fill_background();
    }
    INITIALIZED.store(true, Ordering::Release);
    virtio_gpu::flush(mode.width, mode.height)
}

pub fn is_initialized() -> bool {
    INITIALIZED.load(Ordering::Acquire)
}

pub fn print(arguments: fmt::Arguments<'_>) -> fmt::Result {
    ConsoleWriter.write_fmt(arguments)
}

pub fn print_startup_line(arguments: fmt::Arguments<'_>) -> fmt::Result {
    print(arguments)?;

    let frequency: u64;
    let start: u64;
    unsafe {
        core::arch::asm!(
            "mrs {frequency}, cntfrq_el0",
            "mrs {start}, cntpct_el0",
            frequency = out(reg) frequency,
            start = out(reg) start,
            options(nomem, nostack, preserves_flags)
        );
    }

    if frequency != 0 {
        let delay = frequency / 40;
        loop {
            let current: u64;
            unsafe {
                core::arch::asm!(
                    "mrs {current}, cntpct_el0",
                    current = out(reg) current,
                    options(nomem, nostack, preserves_flags)
                );
            }
            if current.wrapping_sub(start) >= delay {
                break;
            }
            core::hint::spin_loop();
        }
    }

    Ok(())
}

pub fn clear() -> fmt::Result {
    if !is_initialized() {
        return Err(fmt::Error);
    }
    unsafe {
        CURSOR_X = 8;
        CURSOR_Y = 8;
        fill_background();
        virtio_gpu::flush(WIDTH, HEIGHT).map_err(|_| fmt::Error)
    }
}

pub fn draw_startup(
    version: &str,
    heap_kib: usize,
    disk_sectors: Option<u32>,
    filesystem_error: Option<&dyn fmt::Display>,
) -> fmt::Result {
    clear()?;

    let rows = unsafe { HEIGHT / CELL_HEIGHT };
    let separator_row = rows / 5;
    let status_row = rows / 4;
    draw_logo()?;

    let columns = unsafe { WIDTH / CELL_WIDTH };
    let separator = [b'-'; MAX_WIDTH / CELL_WIDTH];
    let separator = core::str::from_utf8(&separator[..columns]).map_err(|_| fmt::Error)?;
    set_cursor(0, separator_row);
    ConsoleWriter.write_str(separator)?;

    set_cursor(0, status_row);
    print_startup_line(format_args!("MoveKernel ver {version}\n"))?;
    print_startup_line(format_args!("ARM MMU identity map enabled\n"))?;
    print_startup_line(format_args!("Heap ready: {heap_kib} KiB\n"))?;
    if let Some(sectors) = disk_sectors {
        print_startup_line(format_args!("Disk filesystem mounted: {sectors} sectors\n"))?;
    } else {
        print_startup_line(format_args!("Disk filesystem unavailable\n"))?;
        if let Some(error) = filesystem_error {
            print_startup_line(format_args!("Filesystem error: {error}\n"))?;
        }
    }

    set_cursor(0, status_row + 4 + usize::from(filesystem_error.is_some()));
    Ok(())
}

pub fn backspace() -> fmt::Result {
    if !is_initialized() {
        return Err(fmt::Error);
    }
    unsafe {
        if CURSOR_X > 8 {
            CURSOR_X -= CELL_WIDTH;
            let buffer = addr_of_mut!(FRAMEBUFFER.0) as *mut u32;
            for row in 0..CELL_HEIGHT {
                for column in 0..CELL_WIDTH {
                    write_volatile(
                        buffer.add((CURSOR_Y + row) * WIDTH + CURSOR_X + column),
                        BACKGROUND,
                    );
                }
            }
        }
        virtio_gpu::flush(WIDTH, HEIGHT).map_err(|_| fmt::Error)
    }
}

fn draw_logo() -> fmt::Result {
    const ART: [&str; 5] = [
        "M   M  OOO    V   V  EEEEE",
        "MM MM O   O   V   V  E    ",
        "M M M O   O   V   V  EEEE ",
        "M   M O   O    V V   E    ",
        "M   M  OOO      V    EEEEE",
    ];
    const SCALE: usize = 2;

    let width = unsafe { WIDTH };
    let height = unsafe { HEIGHT };
    let art_width = ART[0]
        .len()
        .saturating_mul(CELL_WIDTH)
        .saturating_mul(SCALE);
    let origin_x = width.saturating_sub(art_width) / 2;
    let origin_y = 16;
    let buffer = unsafe { addr_of_mut!(FRAMEBUFFER.0) as *mut u32 };

    for (art_row, line) in ART.iter().enumerate() {
        for (character_index, character) in line.chars().enumerate() {
            let glyph = glyph(character);
            let character_x = origin_x + character_index * CELL_WIDTH * SCALE;
            for (row, bits) in glyph.iter().enumerate() {
                for column in 0..5 {
                    if bits & (1 << (4 - column)) != 0 {
                        for dy in 0..SCALE {
                            for dx in 0..SCALE {
                                let x = character_x + column * SCALE + dx;
                                let y = origin_y + (art_row * CELL_HEIGHT + row) * SCALE + dy;
                                if x < width && y < height {
                                    unsafe {
                                        write_volatile(buffer.add(y * width + x), FOREGROUND);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

fn set_cursor(column: usize, row: usize) {
    unsafe {
        CURSOR_X = column.saturating_mul(CELL_WIDTH);
        CURSOR_Y = row.saturating_mul(CELL_HEIGHT);
    }
}

struct ConsoleWriter;

impl Write for ConsoleWriter {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        if !is_initialized() {
            return Err(fmt::Error);
        }

        unsafe {
            for character in text.chars() {
                if character == '\n' {
                    newline();
                } else if character == '\r' {
                    CURSOR_X = 8;
                } else {
                    draw_character(character);
                }
            }

            virtio_gpu::flush(WIDTH, HEIGHT).map_err(|_| fmt::Error)
        }
    }
}

unsafe fn fill_background() {
    unsafe {
        let buffer = addr_of_mut!(FRAMEBUFFER.0) as *mut u32;
        for index in 0..WIDTH * HEIGHT {
            write_volatile(buffer.add(index), BACKGROUND);
        }
    }
}

unsafe fn newline() {
    unsafe {
        CURSOR_X = 8;
        CURSOR_Y += CELL_HEIGHT;
        if CURSOR_Y + CELL_HEIGHT > HEIGHT {
            scroll();
        }
    }
}

unsafe fn scroll() {
    let width = unsafe { WIDTH };
    let height = unsafe { HEIGHT };
    let buffer = unsafe { addr_of_mut!(FRAMEBUFFER.0) as *mut u32 };
    let rows_to_keep = height.saturating_sub(CELL_HEIGHT);
    unsafe {
        copy(
            buffer.add(width * CELL_HEIGHT),
            buffer,
            width * rows_to_keep,
        );
        for index in width * rows_to_keep..width * height {
            write_volatile(buffer.add(index), BACKGROUND);
        }
        CURSOR_Y = rows_to_keep;
    }
}

unsafe fn draw_character(character: char) {
    if unsafe { CURSOR_X + CELL_WIDTH > WIDTH } {
        unsafe { newline() };
    }
    if unsafe { CURSOR_Y + CELL_HEIGHT > HEIGHT } {
        unsafe { scroll() };
    }

    let glyph = glyph(character);
    let x = unsafe { CURSOR_X };
    let y = unsafe { CURSOR_Y };
    let width = unsafe { WIDTH };
    let buffer = unsafe { addr_of_mut!(FRAMEBUFFER.0) as *mut u32 };
    for (row, bits) in glyph.iter().enumerate() {
        for column in 0..5 {
            if bits & (1 << (4 - column)) != 0 {
                unsafe { write_volatile(buffer.add((y + row) * width + x + column), FOREGROUND) };
            }
        }
    }
    unsafe { CURSOR_X += CELL_WIDTH };
}

fn glyph(character: char) -> [u8; 7] {
    let upper = character.to_ascii_uppercase();
    if upper.is_ascii_uppercase() {
        FONT[(upper as u8 - b'A') as usize]
    } else if upper.is_ascii_digit() {
        FONT[26 + (upper as u8 - b'0') as usize]
    } else {
        match character {
            ' ' => [0; 7],
            '.' => [0, 0, 0, 0, 0, 12, 12],
            ',' => [0, 0, 0, 0, 4, 4, 8],
            ':' => [0, 4, 4, 0, 4, 4, 0],
            '-' => [0, 0, 0, 31, 0, 0, 0],
            '_' => [0, 0, 0, 0, 0, 0, 31],
            '!' => [4, 4, 4, 4, 4, 0, 4],
            '?' => [14, 17, 1, 2, 4, 0, 4],
            '=' => [0, 31, 0, 31, 0, 0, 0],
            '<' => [4, 8, 16, 8, 4, 0, 0],
            '>' => [16, 8, 4, 8, 16, 0, 0],
            '\'' => [4, 4, 8, 0, 0, 0, 0],
            '(' => [2, 4, 8, 8, 8, 4, 2],
            ')' => [8, 4, 2, 2, 2, 4, 8],
            '/' => [1, 2, 2, 4, 8, 8, 16],
            _ => [14, 17, 1, 2, 4, 0, 4],
        }
    }
}
