use core::fmt;
use core::mem::size_of;
use core::ptr::{addr_of, addr_of_mut, read_volatile, write_volatile};
use core::sync::atomic::{Ordering, fence};

const MMIO_BASE: usize = 0x0a00_0000;
const MMIO_STRIDE: usize = 0x200;
const MMIO_SLOTS: usize = 32;
const VIRTIO_MAGIC: u32 = 0x7472_6976;
const VIRTIO_DEVICE_BLOCK: u32 = 2;
const QUEUE_SIZE: usize = 8;
const SECTOR_SIZE: usize = 512;

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
const REG_INTERRUPT_ACK: usize = 0x064;
const REG_STATUS: usize = 0x070;
const REG_QUEUE_DESC_LOW: usize = 0x080;
const REG_QUEUE_DESC_HIGH: usize = 0x084;
const REG_QUEUE_DRIVER_LOW: usize = 0x090;
const REG_QUEUE_DRIVER_HIGH: usize = 0x094;
const REG_QUEUE_DEVICE_LOW: usize = 0x0a0;
const REG_QUEUE_DEVICE_HIGH: usize = 0x0a4;
const REG_CONFIG: usize = 0x100;

const FEATURE_VERSION_1: u32 = 1;
const REQUEST_IN: u32 = 0;
const DESCRIPTOR_F_NEXT: u16 = 1;
const DESCRIPTOR_F_WRITE: u16 = 2;

#[derive(Clone, Copy, Debug)]
pub enum Error {
    DeviceNotFound,
    UnsupportedVersion,
    MissingVersionFeature,
    FeatureNegotiationFailed,
    QueueUnavailable,
    InvalidCapacity,
    SectorOutOfRange,
    RequestTimedOut,
    IoError,
    UnsupportedRequest,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DeviceNotFound => formatter.write_str("virtio-blk device not found"),
            Self::UnsupportedVersion => formatter.write_str("unsupported virtio-mmio version"),
            Self::MissingVersionFeature => formatter.write_str("virtio version 1 feature missing"),
            Self::FeatureNegotiationFailed => formatter.write_str("feature negotiation failed"),
            Self::QueueUnavailable => formatter.write_str("virtio-blk queue unavailable"),
            Self::InvalidCapacity => formatter.write_str("virtio-blk reported an empty disk"),
            Self::SectorOutOfRange => formatter.write_str("disk sector is out of range"),
            Self::RequestTimedOut => formatter.write_str("virtio-blk request timed out"),
            Self::IoError => formatter.write_str("virtio-blk read failed"),
            Self::UnsupportedRequest => formatter.write_str("virtio-blk rejected the read request"),
        }
    }
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

#[repr(C, align(16))]
struct RequestHeader {
    request_type: u32,
    reserved: u32,
    sector: u64,
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
static mut HEADER: RequestHeader = RequestHeader {
    request_type: REQUEST_IN,
    reserved: 0,
    sector: 0,
};
#[repr(align(16))]
struct SectorBuffer([u8; SECTOR_SIZE]);
static mut DATA: SectorBuffer = SectorBuffer([0; SECTOR_SIZE]);
static mut STATUS: u8 = 0xff;
static mut DEVICE_BASE: usize = 0;
static mut CAPACITY: u64 = 0;
static mut AVAILABLE_INDEX: u16 = 0;
static mut COMPLETED_INDEX: u16 = 0;

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
            && read32(base, REG_DEVICE_ID) == VIRTIO_DEVICE_BLOCK
        {
            if read32(base, REG_VERSION) != 2 {
                return Err(Error::UnsupportedVersion);
            }
            return Ok(base);
        }
    }
    Err(Error::DeviceNotFound)
}

fn negotiate_features(base: usize) -> Result<(), Error> {
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
    Ok(())
}

fn configure_queue(base: usize) -> Result<(), Error> {
    write32(base, REG_QUEUE_SEL, 0);
    if read32(base, REG_QUEUE_NUM_MAX) < 3 {
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
    write32(base, REG_QUEUE_READY, 1);
    if read32(base, REG_QUEUE_READY) != 1 {
        return Err(Error::QueueUnavailable);
    }
    Ok(())
}

pub fn init() -> Result<(), Error> {
    let base = locate_device()?;
    negotiate_features(base)?;
    configure_queue(base)?;
    write32(base, REG_STATUS, 15);

    let low = read32(base, REG_CONFIG) as u64;
    let high = read32(base, REG_CONFIG + 4) as u64;
    let capacity = (high << 32) | low;
    if capacity == 0 {
        return Err(Error::InvalidCapacity);
    }
    unsafe {
        DEVICE_BASE = base;
        CAPACITY = capacity;
    }
    Ok(())
}

pub fn capacity_sectors() -> u64 {
    unsafe { CAPACITY }
}

pub fn read_sector(sector: u64, output: &mut [u8; SECTOR_SIZE]) -> Result<(), Error> {
    let base = unsafe { DEVICE_BASE };
    if base == 0 {
        return Err(Error::DeviceNotFound);
    }
    if sector >= unsafe { CAPACITY } {
        return Err(Error::SectorOutOfRange);
    }

    let (descriptors, available, used, header, data, status) = unsafe {
        (
            addr_of_mut!(DESCRIPTORS.0) as *mut Descriptor,
            addr_of_mut!(AVAILABLE),
            addr_of!(USED),
            addr_of_mut!(HEADER),
            addr_of_mut!(DATA.0) as *mut u8,
            addr_of_mut!(STATUS),
        )
    };

    unsafe {
        write_volatile(
            header,
            RequestHeader {
                request_type: REQUEST_IN,
                reserved: 0,
                sector,
            },
        );
        write_volatile(status, 0xff);
        write_volatile(
            descriptors,
            Descriptor {
                address: header as u64,
                length: size_of::<RequestHeader>() as u32,
                flags: DESCRIPTOR_F_NEXT,
                next: 1,
            },
        );
        write_volatile(
            descriptors.add(1),
            Descriptor {
                address: data as u64,
                length: SECTOR_SIZE as u32,
                flags: DESCRIPTOR_F_NEXT | DESCRIPTOR_F_WRITE,
                next: 2,
            },
        );
        write_volatile(
            descriptors.add(2),
            Descriptor {
                address: status as u64,
                length: 1,
                flags: DESCRIPTOR_F_WRITE,
                next: 0,
            },
        );

        let slot = (AVAILABLE_INDEX as usize) % QUEUE_SIZE;
        write_volatile(addr_of_mut!((*available).ring[slot]), 0);
        fence(Ordering::Release);
        AVAILABLE_INDEX = AVAILABLE_INDEX.wrapping_add(1);
        write_volatile(addr_of_mut!((*available).index), AVAILABLE_INDEX);
        fence(Ordering::SeqCst);
        write32(base, REG_QUEUE_NOTIFY, 0);

        let mut completed = false;
        for _ in 0..20_000_000 {
            if read_volatile(addr_of!((*used).index)) == COMPLETED_INDEX.wrapping_add(1) {
                completed = true;
                break;
            }
            core::hint::spin_loop();
        }
        if !completed {
            return Err(Error::RequestTimedOut);
        }
        fence(Ordering::Acquire);
        COMPLETED_INDEX = COMPLETED_INDEX.wrapping_add(1);

        let interrupt_status = read32(base, REG_INTERRUPT_ACK);
        if interrupt_status != 0 {
            write32(base, REG_INTERRUPT_ACK, interrupt_status);
        }

        match read_volatile(status) {
            0 => core::ptr::copy_nonoverlapping(data, output.as_mut_ptr(), SECTOR_SIZE),
            1 => return Err(Error::IoError),
            _ => return Err(Error::UnsupportedRequest),
        }
    }
    Ok(())
}
