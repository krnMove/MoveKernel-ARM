use core::arch::asm;
use core::fmt;
use core::ptr::{read_volatile, write_volatile};

const GICD_BASE: usize = 0x0800_0000;
const GICR_BASE: usize = 0x080a_0000;
const GICR_WAKER: usize = 0x0014;

#[derive(Clone, Copy, Debug)]
pub enum Error {
    RedistributorUnavailable,
    SystemRegisterInterfaceUnavailable,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RedistributorUnavailable => formatter.write_str("GIC redistributor did not wake"),
            Self::SystemRegisterInterfaceUnavailable => {
                formatter.write_str("GICv3 system register interface unavailable")
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
        asm!(
            "msr CNTP_CTL_EL0, xzr",
            "isb",
            options(nostack, preserves_flags)
        );

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
    Ok(())
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

    if acknowledge & 0x3ff < 1020 {
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
