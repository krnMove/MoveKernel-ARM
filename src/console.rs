use core::fmt::{self, Write};
use core::ptr::{addr_of_mut, copy, write_volatile};
use core::sync::atomic::{AtomicBool, Ordering};

use crate::virtio_gpu::{self, Error as GpuError};

const MAX_WIDTH: usize = 1280;
const MAX_HEIGHT: usize = 800;
const CELL_WIDTH: usize = 6;
const CELL_HEIGHT: usize = 8;
const BACKGROUND: u32 = 0xff10_1820;
const FOREGROUND: u32 = 0xffd0_d8e0;

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
            '(' => [2, 4, 8, 8, 8, 4, 2],
            ')' => [8, 4, 2, 2, 2, 4, 8],
            '/' => [1, 2, 2, 4, 8, 8, 16],
            _ => [14, 17, 1, 2, 4, 0, 4],
        }
    }
}
