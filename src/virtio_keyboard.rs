use core::fmt;
use core::ptr::{addr_of, addr_of_mut, read_volatile, write_volatile};
use core::sync::atomic::{Ordering, fence};

const MMIO_BASE: usize = 0x0a00_0000;
const MMIO_STRIDE: usize = 0x200;
const MMIO_SLOTS: usize = 32;
const VIRTIO_MAGIC: u32 = 0x7472_6976;
const VIRTIO_DEVICE_INPUT: u32 = 18;
const FEATURE_VERSION_1: u32 = 1;
const QUEUE_SIZE: usize = 8;

const REG_MAGIC: usize = 0x000;
const REG_VERSION: usize = 0x004;
const REG_DEVICE_ID: usize = 0x008;
const REG_DEVICE_FEATURES: usize = 0x010;
const REG_DEVICE_FEATURES_SEL: usize = 0x014;
const REG_DRIVER_FEATURES: usize = 0x020;
const REG_DRIVER_FEATURES_SEL: usize = 0x024;
const REG_QUEUE_SEL: usize = 0x030;
const REG_QUEUE_NUM_MAX: usize = 0x034;
const REG_QUEUE_NUM: usize = 0x038;
const REG_QUEUE_READY: usize = 0x044;
const REG_QUEUE_NOTIFY: usize = 0x050;
const REG_STATUS: usize = 0x070;
const REG_QUEUE_DESC_LOW: usize = 0x080;
const REG_QUEUE_DESC_HIGH: usize = 0x084;
const REG_QUEUE_DRIVER_LOW: usize = 0x090;
const REG_QUEUE_DRIVER_HIGH: usize = 0x094;
const REG_QUEUE_DEVICE_LOW: usize = 0x0a0;
const REG_QUEUE_DEVICE_HIGH: usize = 0x0a4;

const DESCRIPTOR_F_WRITE: u16 = 2;

#[derive(Clone, Copy, Debug)]
pub enum Error {
    DeviceNotFound,
    UnsupportedVersion,
    MissingVersionFeature,
    FeatureNegotiationFailed,
    QueueUnavailable,
    InvalidUsedDescriptor(u32),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DeviceNotFound => {
                formatter.write_str("virtio keyboard not found; add -device virtio-keyboard-device")
            }
            Self::UnsupportedVersion => {
                formatter.write_str("unsupported virtio keyboard transport version")
            }
            Self::MissingVersionFeature => formatter.write_str("virtio keyboard lacks VERSION_1"),
            Self::FeatureNegotiationFailed => {
                formatter.write_str("virtio keyboard feature negotiation failed")
            }
            Self::QueueUnavailable => {
                formatter.write_str("virtio keyboard event queue unavailable")
            }
            Self::InvalidUsedDescriptor(id) => {
                write!(formatter, "invalid keyboard event descriptor {id}")
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Key {
    Character(char),
    Enter,
    Backspace,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Descriptor {
    address: u64,
    length: u32,
    flags: u16,
    next: u16,
}

impl Descriptor {
    const EMPTY: Self = Self {
        address: 0,
        length: 0,
        flags: 0,
        next: 0,
    };
}

#[repr(C)]
#[derive(Clone, Copy)]
struct UsedElement {
    id: u32,
    length: u32,
}

#[repr(C, align(16))]
struct DescriptorTable([Descriptor; QUEUE_SIZE]);

#[repr(C, align(2))]
struct AvailableRing {
    flags: u16,
    index: u16,
    ring: [u16; QUEUE_SIZE],
    used_event: u16,
}

#[repr(C, align(4))]
struct UsedRing {
    flags: u16,
    index: u16,
    ring: [UsedElement; QUEUE_SIZE],
    available_event: u16,
}

#[repr(C, align(8))]
#[derive(Clone, Copy)]
struct InputEvent {
    event_type: u16,
    code: u16,
    value: u32,
}

impl InputEvent {
    const EMPTY: Self = Self {
        event_type: 0,
        code: 0,
        value: 0,
    };
}

static mut DESCRIPTORS: DescriptorTable = DescriptorTable([Descriptor::EMPTY; QUEUE_SIZE]);
static mut AVAILABLE: AvailableRing = AvailableRing {
    flags: 0,
    index: 0,
    ring: [0; QUEUE_SIZE],
    used_event: 0,
};
static mut USED: UsedRing = UsedRing {
    flags: 0,
    index: 0,
    ring: [UsedElement { id: 0, length: 0 }; QUEUE_SIZE],
    available_event: 0,
};
static mut EVENTS: [InputEvent; QUEUE_SIZE] = [InputEvent::EMPTY; QUEUE_SIZE];
static mut DEVICE_BASE: usize = 0;
static mut AVAILABLE_INDEX: u16 = 0;
static mut LAST_USED_INDEX: u16 = 0;
static mut SHIFT: bool = false;
static mut CAPS_LOCK: bool = false;

fn read32(base: usize, register: usize) -> u32 {
    unsafe { read_volatile((base + register) as *const u32) }
}

fn write32(base: usize, register: usize, value: u32) {
    unsafe { write_volatile((base + register) as *mut u32, value) }
}

fn locate_device() -> Result<usize, Error> {
    for slot in 0..MMIO_SLOTS {
        let base = MMIO_BASE + slot * MMIO_STRIDE;
        if read32(base, REG_MAGIC) == VIRTIO_MAGIC
            && read32(base, REG_DEVICE_ID) == VIRTIO_DEVICE_INPUT
        {
            if read32(base, REG_VERSION) != 2 {
                return Err(Error::UnsupportedVersion);
            }
            return Ok(base);
        }
    }
    Err(Error::DeviceNotFound)
}

fn configure_queue(base: usize) -> Result<(), Error> {
    write32(base, REG_QUEUE_SEL, 0);
    if read32(base, REG_QUEUE_NUM_MAX) < QUEUE_SIZE as u32 {
        return Err(Error::QueueUnavailable);
    }
    write32(base, REG_QUEUE_NUM, QUEUE_SIZE as u32);
    let (descriptors, available, used) = unsafe {
        (
            addr_of_mut!(DESCRIPTORS.0) as u64,
            addr_of_mut!(AVAILABLE) as u64,
            addr_of_mut!(USED) as u64,
        )
    };
    write32(base, REG_QUEUE_DESC_LOW, descriptors as u32);
    write32(base, REG_QUEUE_DESC_HIGH, (descriptors >> 32) as u32);
    write32(base, REG_QUEUE_DRIVER_LOW, available as u32);
    write32(base, REG_QUEUE_DRIVER_HIGH, (available >> 32) as u32);
    write32(base, REG_QUEUE_DEVICE_LOW, used as u32);
    write32(base, REG_QUEUE_DEVICE_HIGH, (used >> 32) as u32);

    let events = addr_of_mut!(EVENTS) as *mut InputEvent;
    let descriptors = unsafe { addr_of_mut!(DESCRIPTORS.0) as *mut Descriptor };
    let available_ring = addr_of_mut!(AVAILABLE);
    for index in 0..QUEUE_SIZE {
        unsafe {
            write_volatile(
                descriptors.add(index),
                Descriptor {
                    address: events.add(index) as u64,
                    length: core::mem::size_of::<InputEvent>() as u32,
                    flags: DESCRIPTOR_F_WRITE,
                    next: 0,
                },
            );
            write_volatile(addr_of_mut!((*available_ring).ring[index]), index as u16);
        }
    }
    fence(Ordering::Release);
    unsafe {
        AVAILABLE_INDEX = QUEUE_SIZE as u16;
        write_volatile(addr_of_mut!((*available_ring).index), AVAILABLE_INDEX);
    }
    write32(base, REG_QUEUE_READY, 1);
    if read32(base, REG_QUEUE_READY) != 1 {
        return Err(Error::QueueUnavailable);
    }
    write32(base, REG_QUEUE_NOTIFY, 0);
    Ok(())
}

pub fn init() -> Result<(), Error> {
    let base = locate_device()?;
    write32(base, REG_STATUS, 0);
    write32(base, REG_STATUS, 1);
    write32(base, REG_STATUS, 3);

    write32(base, REG_DEVICE_FEATURES_SEL, 1);
    if read32(base, REG_DEVICE_FEATURES) & FEATURE_VERSION_1 == 0 {
        write32(base, REG_STATUS, 0x80);
        return Err(Error::MissingVersionFeature);
    }

    write32(base, REG_DRIVER_FEATURES_SEL, 0);
    write32(base, REG_DRIVER_FEATURES, 0);
    write32(base, REG_DRIVER_FEATURES_SEL, 1);
    write32(base, REG_DRIVER_FEATURES, FEATURE_VERSION_1);
    write32(base, REG_STATUS, 11);
    if read32(base, REG_STATUS) & 8 == 0 {
        write32(base, REG_STATUS, read32(base, REG_STATUS) | 0x80);
        return Err(Error::FeatureNegotiationFailed);
    }

    configure_queue(base)?;
    write32(base, REG_STATUS, 15);
    unsafe { DEVICE_BASE = base };
    Ok(())
}

pub fn poll_key() -> Result<Option<Key>, Error> {
    let base = unsafe { DEVICE_BASE };
    if base == 0 {
        return Err(Error::DeviceNotFound);
    }

    loop {
        let (used, available, events, last_used_index, available_index) = unsafe {
            (
                addr_of!(USED),
                addr_of_mut!(AVAILABLE),
                addr_of_mut!(EVENTS) as *mut InputEvent,
                LAST_USED_INDEX,
                AVAILABLE_INDEX,
            )
        };
        let used_index = unsafe { read_volatile(addr_of!((*used).index)) };
        if last_used_index == used_index {
            return Ok(None);
        }
        fence(Ordering::Acquire);

        let used_slot = last_used_index as usize % QUEUE_SIZE;
        let descriptor_id = unsafe { read_volatile(addr_of!((*used).ring[used_slot].id)) };
        if descriptor_id as usize >= QUEUE_SIZE {
            return Err(Error::InvalidUsedDescriptor(descriptor_id));
        }
        let event = unsafe { read_volatile(events.add(descriptor_id as usize)) };

        let available_slot = available_index as usize % QUEUE_SIZE;
        unsafe {
            write_volatile(
                addr_of_mut!((*available).ring[available_slot]),
                descriptor_id as u16,
            );
            fence(Ordering::Release);
            AVAILABLE_INDEX = available_index.wrapping_add(1);
            write_volatile(addr_of_mut!((*available).index), AVAILABLE_INDEX);
        }
        write32(base, REG_QUEUE_NOTIFY, 0);
        unsafe {
            LAST_USED_INDEX = last_used_index.wrapping_add(1);
        }

        if event.event_type != 1 {
            continue;
        }
        if event.code == 42 || event.code == 54 {
            unsafe { SHIFT = event.value != 0 };
            continue;
        }
        if event.code == 58 && event.value == 1 {
            unsafe { CAPS_LOCK = !CAPS_LOCK };
            continue;
        }
        if event.value == 0 {
            continue;
        }
        if event.code == 28 {
            return Ok(Some(Key::Enter));
        }
        if event.code == 14 {
            return Ok(Some(Key::Backspace));
        }

        if let Some(character) = key_character(event.code) {
            let uppercase = unsafe { SHIFT ^ CAPS_LOCK };
            return Ok(Some(Key::Character(if uppercase {
                character.to_ascii_uppercase()
            } else {
                character
            })));
        }
    }
}

fn key_character(code: u16) -> Option<char> {
    Some(match code {
        2..=10 => {
            let normal = (b'1' + (code as u8 - 2)) as char;
            if unsafe { SHIFT } {
                match code {
                    2 => '!',
                    3 => '@',
                    4 => '#',
                    5 => '$',
                    6 => '%',
                    7 => '^',
                    8 => '&',
                    9 => '*',
                    _ => '(',
                }
            } else {
                normal
            }
        }
        11 => {
            if unsafe { SHIFT } {
                ')'
            } else {
                '0'
            }
        }
        12 => {
            if unsafe { SHIFT } {
                '_'
            } else {
                '-'
            }
        }
        13 => {
            if unsafe { SHIFT } {
                '+'
            } else {
                '='
            }
        }
        16 => 'q',
        17 => 'w',
        18 => 'e',
        19 => 'r',
        20 => 't',
        21 => 'y',
        22 => 'u',
        23 => 'i',
        24 => 'o',
        25 => 'p',
        26 => {
            if unsafe { SHIFT } {
                '{'
            } else {
                '['
            }
        }
        27 => {
            if unsafe { SHIFT } {
                '}'
            } else {
                ']'
            }
        }
        30 => 'a',
        31 => 's',
        32 => 'd',
        33 => 'f',
        34 => 'g',
        35 => 'h',
        36 => 'j',
        37 => 'k',
        38 => 'l',
        39 => {
            if unsafe { SHIFT } {
                ':'
            } else {
                ';'
            }
        }
        40 => {
            if unsafe { SHIFT } {
                '"'
            } else {
                '\''
            }
        }
        41 => {
            if unsafe { SHIFT } {
                '~'
            } else {
                '`'
            }
        }
        43 => {
            if unsafe { SHIFT } {
                '|'
            } else {
                '\\'
            }
        }
        44 => 'z',
        45 => 'x',
        46 => 'c',
        47 => 'v',
        48 => 'b',
        49 => 'n',
        50 => 'm',
        51 => {
            if unsafe { SHIFT } {
                '<'
            } else {
                ','
            }
        }
        52 => {
            if unsafe { SHIFT } {
                '>'
            } else {
                '.'
            }
        }
        53 => {
            if unsafe { SHIFT } {
                '?'
            } else {
                '/'
            }
        }
        57 => ' ',
        _ => return None,
    })
}
