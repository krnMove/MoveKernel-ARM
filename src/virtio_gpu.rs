use core::fmt;
use core::mem::size_of;
use core::ptr::{addr_of, addr_of_mut, read_volatile, write_volatile};
use core::sync::atomic::{Ordering, fence};

const MMIO_BASE: usize = 0x0a00_0000;
const MMIO_STRIDE: usize = 0x200;
const MMIO_SLOTS: usize = 32;
const VIRTIO_MAGIC: u32 = 0x7472_6976;
const VIRTIO_DEVICE_GPU: u32 = 16;
const VIRTIO_F_VERSION_1_HIGH_BIT: u32 = 0;
const QUEUE_SIZE: usize = 8;
const RESOURCE_ID: u32 = 1;

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

const CMD_GET_DISPLAY_INFO: u32 = 0x0100;
const CMD_RESOURCE_CREATE_2D: u32 = 0x0101;
const CMD_SET_SCANOUT: u32 = 0x0103;
const CMD_RESOURCE_FLUSH: u32 = 0x0104;
const CMD_TRANSFER_TO_HOST_2D: u32 = 0x0105;
const CMD_RESOURCE_ATTACH_BACKING: u32 = 0x0106;
const RESP_OK_NODATA: u32 = 0x1100;
const RESP_OK_DISPLAY_INFO: u32 = 0x1101;
const FORMAT_B8G8R8A8_UNORM: u32 = 1;
const DESCRIPTOR_F_NEXT: u16 = 1;
const DESCRIPTOR_F_WRITE: u16 = 2;

#[derive(Clone, Copy, Debug)]
pub enum Error {
    DeviceNotFound,
    UnsupportedVersion,
    MissingVersionFeature,
    FeatureNegotiationFailed,
    QueueUnavailable,
    NoDisplay,
    DisplayTooLarge,
    RequestTimedOut,
    DeviceRejectedRequest(u32),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DeviceRejectedRequest(response) => {
                write!(formatter, "device returned response {response:#x}")
            }
            Self::DeviceNotFound => formatter.write_str("virtio-gpu device not found"),
            Self::UnsupportedVersion => formatter.write_str("unsupported virtio-mmio version"),
            Self::MissingVersionFeature => formatter.write_str("virtio version 1 feature missing"),
            Self::FeatureNegotiationFailed => formatter.write_str("feature negotiation failed"),
            Self::QueueUnavailable => formatter.write_str("virtqueue unavailable"),
            Self::NoDisplay => formatter.write_str("QEMU reported no active display"),
            Self::DisplayTooLarge => formatter.write_str("QEMU display exceeds framebuffer limits"),
            Self::RequestTimedOut => formatter.write_str("virtio-gpu request timed out"),
        }
    }
}

#[derive(Clone, Copy)]
pub struct DisplayMode {
    pub width: usize,
    pub height: usize,
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
struct CommandBuffer([u8; 64]);

#[repr(C, align(16))]
struct ResponseBuffer([u8; 512]);

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
static mut COMMAND: CommandBuffer = CommandBuffer([0; 64]);
static mut RESPONSE: ResponseBuffer = ResponseBuffer([0; 512]);
static mut DEVICE_BASE: usize = 0;
static mut AVAILABLE_INDEX: u16 = 0;
static mut COMPLETED_INDEX: u16 = 0;

#[repr(C)]
#[derive(Clone, Copy)]
struct ControlHeader {
    command_type: u32,
    flags: u32,
    fence_id: u64,
    context_id: u32,
    padding: u32,
}

impl ControlHeader {
    const fn new(command_type: u32) -> Self {
        Self {
            command_type,
            flags: 0,
            fence_id: 0,
            context_id: 0,
            padding: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Rect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct DisplayModeEntry {
    rect: Rect,
    enabled: u32,
    flags: u32,
}

#[repr(C)]
struct DisplayInfoResponse {
    header: ControlHeader,
    modes: [DisplayModeEntry; 16],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CreateResourceCommand {
    header: ControlHeader,
    resource_id: u32,
    format: u32,
    width: u32,
    height: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct AttachBackingCommand {
    header: ControlHeader,
    resource_id: u32,
    entry_count: u32,
    address: u64,
    length: u32,
    padding: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ScanoutCommand {
    header: ControlHeader,
    rect: Rect,
    scanout_id: u32,
    resource_id: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct TransferCommand {
    header: ControlHeader,
    rect: Rect,
    offset: u64,
    resource_id: u32,
    padding: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct FlushCommand {
    header: ControlHeader,
    rect: Rect,
    resource_id: u32,
    padding: u32,
}

fn mmio_read(base: usize, register: usize) -> u32 {
    unsafe { read_volatile((base + register) as *const u32) }
}

fn mmio_write(base: usize, register: usize, value: u32) {
    unsafe { write_volatile((base + register) as *mut u32, value) }
}

fn locate_device() -> Result<usize, Error> {
    for slot in 0..MMIO_SLOTS {
        let base = MMIO_BASE + slot * MMIO_STRIDE;
        if mmio_read(base, REG_MAGIC) == VIRTIO_MAGIC
            && mmio_read(base, REG_DEVICE_ID) == VIRTIO_DEVICE_GPU
        {
            if mmio_read(base, REG_VERSION) != 2 {
                return Err(Error::UnsupportedVersion);
            }
            return Ok(base);
        }
    }
    Err(Error::DeviceNotFound)
}

fn negotiate_features(base: usize) -> Result<(), Error> {
    mmio_write(base, REG_STATUS, 0);
    mmio_write(base, REG_STATUS, 1);
    mmio_write(base, REG_STATUS, 3);

    mmio_write(base, REG_DEVICE_FEATURES_SEL, 1);
    let high_features = mmio_read(base, REG_DEVICE_FEATURES);
    let version_1_feature = 1 << VIRTIO_F_VERSION_1_HIGH_BIT;
    if high_features & version_1_feature == 0 {
        mmio_write(base, REG_STATUS, 0x80);
        return Err(Error::MissingVersionFeature);
    }

    mmio_write(base, REG_DRIVER_FEATURES_SEL, 0);
    mmio_write(base, REG_DRIVER_FEATURES, 0);
    mmio_write(base, REG_DRIVER_FEATURES_SEL, 1);
    mmio_write(base, REG_DRIVER_FEATURES, version_1_feature);

    mmio_write(base, REG_STATUS, 11);
    if mmio_read(base, REG_STATUS) & 8 == 0 {
        mmio_write(base, REG_STATUS, mmio_read(base, REG_STATUS) | 0x80);
        return Err(Error::FeatureNegotiationFailed);
    }

    Ok(())
}

fn configure_queue(base: usize) -> Result<(), Error> {
    mmio_write(base, REG_QUEUE_SEL, 0);
    let max_size = mmio_read(base, REG_QUEUE_NUM_MAX) as usize;
    if max_size < 2 {
        return Err(Error::QueueUnavailable);
    }

    mmio_write(base, REG_QUEUE_NUM, QUEUE_SIZE as u32);
    let (desc, available, used) = unsafe {
        (
            addr_of_mut!(DESCRIPTORS.0) as u64,
            addr_of_mut!(AVAILABLE) as u64,
            addr_of_mut!(USED) as u64,
        )
    };
    mmio_write(base, REG_QUEUE_DESC_LOW, desc as u32);
    mmio_write(base, REG_QUEUE_DESC_HIGH, (desc >> 32) as u32);
    mmio_write(base, REG_QUEUE_DRIVER_LOW, available as u32);
    mmio_write(base, REG_QUEUE_DRIVER_HIGH, (available >> 32) as u32);
    mmio_write(base, REG_QUEUE_DEVICE_LOW, used as u32);
    mmio_write(base, REG_QUEUE_DEVICE_HIGH, (used >> 32) as u32);
    mmio_write(base, REG_QUEUE_READY, 1);
    if mmio_read(base, REG_QUEUE_READY) != 1 {
        return Err(Error::QueueUnavailable);
    }
    Ok(())
}

fn submit(command: *const u8, command_len: usize) -> Result<*const u8, Error> {
    let base = unsafe { DEVICE_BASE };
    let (descriptors, available, used, response) = unsafe {
        (
            addr_of_mut!(DESCRIPTORS.0) as *mut Descriptor,
            addr_of_mut!(AVAILABLE),
            addr_of!(USED),
            addr_of_mut!(RESPONSE.0) as *mut u8,
        )
    };

    unsafe {
        write_volatile(
            descriptors,
            Descriptor {
                address: command as u64,
                length: command_len as u32,
                flags: DESCRIPTOR_F_NEXT,
                next: 1,
            },
        );
        write_volatile(
            descriptors.add(1),
            Descriptor {
                address: response as u64,
                length: size_of::<ResponseBuffer>() as u32,
                flags: DESCRIPTOR_F_WRITE,
                next: 0,
            },
        );
        write_volatile(response.cast::<u32>(), 0);

        let slot = (AVAILABLE_INDEX as usize) % QUEUE_SIZE;
        write_volatile(addr_of_mut!((*available).ring[slot]), 0);
        fence(Ordering::Release);
        AVAILABLE_INDEX = AVAILABLE_INDEX.wrapping_add(1);
        write_volatile(addr_of_mut!((*available).index), AVAILABLE_INDEX);
        fence(Ordering::SeqCst);
        mmio_write(base, REG_QUEUE_NOTIFY, 0);

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
        let interrupt_status = mmio_read(base, REG_INTERRUPT_ACK);
        if interrupt_status != 0 {
            mmio_write(base, REG_INTERRUPT_ACK, interrupt_status);
        }
    }

    Ok(response)
}

fn send_command<C>(command: &C, expected_response: u32) -> Result<*const u8, Error> {
    let response = submit(command as *const C as *const u8, size_of::<C>())?;
    let response_type = unsafe { read_volatile(response.cast::<u32>()) };
    if response_type != expected_response {
        return Err(Error::DeviceRejectedRequest(response_type));
    }
    Ok(response)
}

fn command_buffer<C: Copy>(command: C) -> Result<(), Error> {
    let storage = unsafe { addr_of_mut!(COMMAND.0) as *mut u8 };
    unsafe {
        core::ptr::write(storage.cast::<C>(), command);
        send_command(&*storage.cast::<C>(), RESP_OK_NODATA)?;
    }
    Ok(())
}

pub fn initialize(
    framebuffer: *mut u32,
    max_width: usize,
    max_height: usize,
) -> Result<DisplayMode, Error> {
    let base = locate_device()?;
    unsafe { DEVICE_BASE = base };
    negotiate_features(base)?;
    configure_queue(base)?;
    mmio_write(base, REG_STATUS, 15);

    let query = ControlHeader::new(CMD_GET_DISPLAY_INFO);
    let response = send_command(&query, RESP_OK_DISPLAY_INFO)?;
    let info = unsafe { &*response.cast::<DisplayInfoResponse>() };
    let mode = info.modes[0];
    if mode.enabled == 0 || mode.rect.width == 0 || mode.rect.height == 0 {
        return Err(Error::NoDisplay);
    }
    let width = mode.rect.width as usize;
    let height = mode.rect.height as usize;
    if width > max_width || height > max_height {
        return Err(Error::DisplayTooLarge);
    }
    let byte_length = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(size_of::<u32>()))
        .ok_or(Error::DisplayTooLarge)?;
    if byte_length > u32::MAX as usize {
        return Err(Error::DisplayTooLarge);
    }

    let create = CreateResourceCommand {
        header: ControlHeader::new(CMD_RESOURCE_CREATE_2D),
        resource_id: RESOURCE_ID,
        format: FORMAT_B8G8R8A8_UNORM,
        width: width as u32,
        height: height as u32,
    };
    command_buffer(create)?;

    let attach = AttachBackingCommand {
        header: ControlHeader::new(CMD_RESOURCE_ATTACH_BACKING),
        resource_id: RESOURCE_ID,
        entry_count: 1,
        address: framebuffer as u64,
        length: byte_length as u32,
        padding: 0,
    };
    command_buffer(attach)?;

    let rect = Rect {
        x: 0,
        y: 0,
        width: width as u32,
        height: height as u32,
    };
    let scanout = ScanoutCommand {
        header: ControlHeader::new(CMD_SET_SCANOUT),
        rect,
        scanout_id: 0,
        resource_id: RESOURCE_ID,
    };
    command_buffer(scanout)?;
    transfer_and_flush(rect)?;

    Ok(DisplayMode { width, height })
}

fn transfer_and_flush(rect: Rect) -> Result<(), Error> {
    let transfer = TransferCommand {
        header: ControlHeader::new(CMD_TRANSFER_TO_HOST_2D),
        rect,
        offset: 0,
        resource_id: RESOURCE_ID,
        padding: 0,
    };
    command_buffer(transfer)?;

    let flush = FlushCommand {
        header: ControlHeader::new(CMD_RESOURCE_FLUSH),
        rect,
        resource_id: RESOURCE_ID,
        padding: 0,
    };
    command_buffer(flush)
}

pub fn flush(width: usize, height: usize) -> Result<(), Error> {
    transfer_and_flush(Rect {
        x: 0,
        y: 0,
        width: width as u32,
        height: height as u32,
    })
}
