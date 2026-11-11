use core::fmt;
use core::ptr::addr_of_mut;
use core::sync::atomic::{AtomicU64, Ordering};

const TASK_COUNT: usize = 2;
const STACK_SIZE: usize = 16 * 1024;
const TASK_STACK_BYTES: usize = STACK_SIZE * TASK_COUNT;

unsafe extern "C" {
    fn context_switch(old: *mut Context, new: *const Context);
    fn task_bootstrap();
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Context {
    callee_saved: [u64; 12],
    stack_pointer: u64,
    simd_saved: [u64; 8],
}

impl Context {
    const fn empty() -> Self {
        Self {
            callee_saved: [0; 12],
            stack_pointer: 0,
            simd_saved: [0; 8],
        }
    }
}

#[repr(C, align(16))]
struct TaskStacks([u8; TASK_STACK_BYTES]);

static mut TASKS: [Context; TASK_COUNT] = [Context::empty(); TASK_COUNT];
static mut STACKS: TaskStacks = TaskStacks([0; TASK_STACK_BYTES]);
static mut SCHEDULER_CONTEXT: Context = Context::empty();
static mut CURRENT_TASK: usize = 0;
static mut NEXT_TASK: usize = 0;
static mut TASK_COUNT_ACTIVE: usize = 0;
static INITIALIZED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static COUNTERS: [AtomicU64; TASK_COUNT] = [const { AtomicU64::new(0) }; TASK_COUNT];

#[derive(Clone, Copy, Debug)]
pub enum Error {
    AlreadyInitialized,
    TaskLimitReached,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyInitialized => formatter.write_str("scheduler already initialized"),
            Self::TaskLimitReached => formatter.write_str("scheduler task limit reached"),
        }
    }
}

pub fn init() -> Result<(), Error> {
    if INITIALIZED.swap(true, Ordering::AcqRel) {
        return Err(Error::AlreadyInitialized);
    }
    add_task(worker_one)?;
    add_task(worker_two)?;
    Ok(())
}

pub fn add_task(task: fn() -> !) -> Result<usize, Error> {
    if !INITIALIZED.load(Ordering::Acquire) {
        INITIALIZED.store(true, Ordering::Release);
    }

    unsafe {
        if TASK_COUNT_ACTIVE >= TASK_COUNT {
            return Err(Error::TaskLimitReached);
        }
        let task_index = TASK_COUNT_ACTIVE;
        let stack_start = addr_of_mut!(STACKS.0) as *mut u8;
        let stack_top = stack_start.add((task_index + 1) * STACK_SIZE) as usize & !0xf;
        let context = addr_of_mut!(TASKS[task_index]);
        (*context).callee_saved[0] = task as usize as u64;
        (*context).callee_saved[11] = task_bootstrap as *const () as usize as u64;
        (*context).stack_pointer = stack_top as u64;
        TASK_COUNT_ACTIVE += 1;
        Ok(task_index)
    }
}

pub fn run_one() {
    if !INITIALIZED.load(Ordering::Acquire) {
        return;
    }
    unsafe {
        if TASK_COUNT_ACTIVE == 0 {
            return;
        }
        let task_index = NEXT_TASK % TASK_COUNT_ACTIVE;
        NEXT_TASK = (task_index + 1) % TASK_COUNT_ACTIVE;
        CURRENT_TASK = task_index;
        context_switch(
            addr_of_mut!(SCHEDULER_CONTEXT),
            addr_of_mut!(TASKS[task_index]),
        );
    }
}

pub fn yield_now() {
    unsafe {
        context_switch(
            addr_of_mut!(TASKS[CURRENT_TASK]),
            addr_of_mut!(SCHEDULER_CONTEXT),
        );
    }
}

pub fn task_count() -> usize {
    unsafe { TASK_COUNT_ACTIVE }
}

pub fn task_runs(index: usize) -> Option<u64> {
    COUNTERS
        .get(index)
        .map(|counter| counter.load(Ordering::Relaxed))
}

fn worker_one() -> ! {
    loop {
        COUNTERS[0].fetch_add(1, Ordering::Relaxed);
        yield_now();
    }
}

fn worker_two() -> ! {
    loop {
        COUNTERS[1].fetch_add(1, Ordering::Relaxed);
        yield_now();
    }
}
