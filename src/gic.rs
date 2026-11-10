use core::arch::asm;
use core::fmt;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicU64, Ordering};

const GICD_BASE: usize = 0x0800_0000;
const GICR_BASE: usize = 0x080a_0000;
const GICR_WAKER: usize = 0x0014;
const GICR_SGI_BASE: usize = 0x1_0000;
const GICR_IGROUPR0: usize = GICR_SGI_BASE + 0x0080;
const GICR_ISENABLER0: usize = GICR_SGI_BASE + 0x0100;
const GICR_IPRIORITYR: usize = GICR_SGI_BASE + 0x0400;
const IRQ_TIMER_PHYSICAL: u32 = 30;
const MAX_TIMER_INTERVAL: u64 = i32::MAX as u64;

static TICKS: AtomicU64 = AtomicU64::new(0);
static mut TIMER_INTERVAL: u32 = 0;

#[derive(Clone, Copy, Debug)]
pub enum Error {
    RedistributorUnavailable,
    SystemRegisterInterfaceUnavailable,
    InvalidTimerFrequency,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RedistributorUnavailable => formatter.write_str("GIC redistributor did not wake"),
            Self::SystemRegisterInterfaceUnavailable => {
                formatter.write_str("GICv3 system register interface unavailable")
            }
            Self::InvalidTimerFrequency => {
                formatter.write_str("invalid ARM generic timer frequency")
            }
        }
    }
}

fn read32(address: usize) -> u32 {
    unsafe { read_volatile(address as *const u32) }
}

fn write32(address: usize, value: u32) {
    unsafe { write_volatile(address as *mut u32, value) }
}

pub fn init() -> Result<(), Error> {
    unsafe {
        let waker = read32(GICR_BASE + GICR_WAKER);
        write32(GICR_BASE + GICR_WAKER, waker & !(1 << 1));

        let mut awake = false;
        for _ in 0..1_000_000 {
            if read32(GICR_BASE + GICR_WAKER) & (1 << 2) == 0 {
                awake = true;
                break;
            }
            core::hint::spin_loop();
        }
        if !awake {
            return Err(Error::RedistributorUnavailable);
        }

        asm!(
            "msr ICC_SRE_EL1, {value}",
            "isb",
            value = in(reg) 1u64,
            options(nostack, preserves_flags)
        );
        let sre: u64;
        asm!("mrs {value}, ICC_SRE_EL1", value = out(reg) sre, options(nomem, nostack, preserves_flags));
        if sre & 1 == 0 {
            return Err(Error::SystemRegisterInterfaceUnavailable);
        }

        let group = read32(GICR_BASE + GICR_IGROUPR0) | (1 << IRQ_TIMER_PHYSICAL);
        write32(GICR_BASE + GICR_IGROUPR0, group);
        write_volatile(
            (GICR_BASE + GICR_IPRIORITYR + IRQ_TIMER_PHYSICAL as usize) as *mut u8,
            0x80,
        );
        write32(GICR_BASE + GICR_ISENABLER0, 1 << IRQ_TIMER_PHYSICAL);

        let distributor_control = read32(GICD_BASE) | (1 << 1);
        write32(GICD_BASE, distributor_control);

        asm!(
            "msr ICC_PMR_EL1, {priority}",
            "msr ICC_BPR1_EL1, xzr",
            "msr ICC_IGRPEN1_EL1, {enable}",
            "isb",
            priority = in(reg) 0xffu64,
            enable = in(reg) 1u64,
            options(nostack, preserves_flags)
        );
    }

    let frequency: u64;
    unsafe {
        asm!(
            "mrs {frequency}, CNTFRQ_EL0",
            frequency = out(reg) frequency,
            options(nomem, nostack, preserves_flags)
        );
    }
    if frequency == 0 {
        return Err(Error::InvalidTimerFrequency);
    }

    let interval = frequency.min(MAX_TIMER_INTERVAL) as u32;
    unsafe {
        TIMER_INTERVAL = interval;
        asm!(
            "msr CNTP_CTL_EL0, xzr",
            "msr CNTP_TVAL_EL0, {interval}",
            "msr CNTP_CTL_EL0, {enable}",
            "isb",
            interval = in(reg) interval as u64,
            enable = in(reg) 1u64,
            options(nostack, preserves_flags)
        );
        asm!("msr DAIFClr, #2", options(nostack, preserves_flags));
    }

    Ok(())
}

pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

#[unsafe(no_mangle)]
pub extern "C" fn gic_irq_handler() {
    let acknowledge: u64;
    unsafe {
        asm!(
            "mrs {value}, ICC_IAR1_EL1",
            value = out(reg) acknowledge,
            options(nomem, nostack, preserves_flags)
        );
    }

    let interrupt_id = (acknowledge & 0x3ff) as u32;
    if interrupt_id == IRQ_TIMER_PHYSICAL {
        TICKS.fetch_add(1, Ordering::Relaxed);
        let interval = unsafe { TIMER_INTERVAL };
        unsafe {
            asm!(
                "msr CNTP_TVAL_EL0, {interval}",
                interval = in(reg) interval as u64,
                options(nostack, preserves_flags)
            );
        }
    }

    if interrupt_id < 1020 {
        unsafe {
            asm!(
                "msr ICC_EOIR1_EL1, {value}",
                "isb",
                value = in(reg) acknowledge,
                options(nostack, preserves_flags)
            );
        }
    }
}
