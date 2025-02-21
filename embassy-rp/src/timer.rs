use atomic_polyfill::{AtomicU8, Ordering};
use core::cell::Cell;
use critical_section::CriticalSection;
use embassy::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy::blocking_mutex::Mutex;
use embassy::interrupt::{Interrupt, InterruptExt};
use embassy::time::driver::{AlarmHandle, Driver};

use crate::{interrupt, pac};

struct AlarmState {
    timestamp: Cell<u64>,
    callback: Cell<Option<(fn(*mut ()), *mut ())>>,
}
unsafe impl Send for AlarmState {}

const ALARM_COUNT: usize = 4;
const DUMMY_ALARM: AlarmState = AlarmState {
    timestamp: Cell::new(0),
    callback: Cell::new(None),
};

struct TimerDriver {
    alarms: Mutex<CriticalSectionRawMutex, [AlarmState; ALARM_COUNT]>,
    next_alarm: AtomicU8,
}

embassy::time_driver_impl!(static DRIVER: TimerDriver = TimerDriver{
    alarms:  Mutex::const_new(CriticalSectionRawMutex::new(), [DUMMY_ALARM; ALARM_COUNT]),
    next_alarm: AtomicU8::new(0),
});

impl Driver for TimerDriver {
    fn now(&self) -> u64 {
        loop {
            unsafe {
                let hi = pac::TIMER.timerawh().read();
                let lo = pac::TIMER.timerawl().read();
                let hi2 = pac::TIMER.timerawh().read();
                if hi == hi2 {
                    return (hi as u64) << 32 | (lo as u64);
                }
            }
        }
    }

    unsafe fn allocate_alarm(&self) -> Option<AlarmHandle> {
        let id = self
            .next_alarm
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |x| {
                if x < ALARM_COUNT as u8 {
                    Some(x + 1)
                } else {
                    None
                }
            });

        match id {
            Ok(id) => Some(AlarmHandle::new(id)),
            Err(_) => None,
        }
    }

    fn set_alarm_callback(&self, alarm: AlarmHandle, callback: fn(*mut ()), ctx: *mut ()) {
        let n = alarm.id() as usize;
        critical_section::with(|cs| {
            let alarm = &self.alarms.borrow(cs)[n];
            alarm.callback.set(Some((callback, ctx)));
        })
    }

    fn set_alarm(&self, alarm: AlarmHandle, timestamp: u64) {
        let n = alarm.id() as usize;
        critical_section::with(|cs| {
            let alarm = &self.alarms.borrow(cs)[n];
            alarm.timestamp.set(timestamp);

            // Arm it.
            // Note that we're not checking the high bits at all. This means the irq may fire early
            // if the alarm is more than 72 minutes (2^32 us) in the future. This is OK, since on irq fire
            // it is checked if the alarm time has passed.
            unsafe { pac::TIMER.alarm(n).write_value(timestamp as u32) };

            let now = self.now();

            // If alarm timestamp has passed, trigger it instantly.
            // This disarms it.
            if timestamp <= now {
                self.trigger_alarm(n, cs);
            }
        })
    }
}

impl TimerDriver {
    fn check_alarm(&self, n: usize) {
        critical_section::with(|cs| {
            let alarm = &self.alarms.borrow(cs)[n];
            let timestamp = alarm.timestamp.get();
            if timestamp <= self.now() {
                self.trigger_alarm(n, cs)
            } else {
                // Not elapsed, arm it again.
                // This can happen if it was set more than 2^32 us in the future.
                unsafe { pac::TIMER.alarm(n).write_value(timestamp as u32) };
            }
        });

        // clear the irq
        unsafe { pac::TIMER.intr().write(|w| w.set_alarm(n, true)) }
    }

    fn trigger_alarm(&self, n: usize, cs: CriticalSection) {
        // disarm
        unsafe { pac::TIMER.armed().write(|w| w.set_armed(1 << n)) }

        let alarm = &self.alarms.borrow(cs)[n];
        alarm.timestamp.set(u64::MAX);

        // Call after clearing alarm, so the callback can set another alarm.
        if let Some((f, ctx)) = alarm.callback.get() {
            f(ctx);
        }
    }
}

/// safety: must be called exactly once at bootup
pub unsafe fn init() {
    // init alarms
    critical_section::with(|cs| {
        let alarms = DRIVER.alarms.borrow(cs);
        for a in alarms {
            a.timestamp.set(u64::MAX);
        }
    });

    // enable all irqs
    pac::TIMER.inte().write(|w| {
        w.set_alarm(0, true);
        w.set_alarm(1, true);
        w.set_alarm(2, true);
        w.set_alarm(3, true);
    });
    interrupt::TIMER_IRQ_0::steal().enable();
    interrupt::TIMER_IRQ_1::steal().enable();
    interrupt::TIMER_IRQ_2::steal().enable();
    interrupt::TIMER_IRQ_3::steal().enable();
}

#[interrupt]
unsafe fn TIMER_IRQ_0() {
    DRIVER.check_alarm(0)
}

#[interrupt]
unsafe fn TIMER_IRQ_1() {
    DRIVER.check_alarm(1)
}

#[interrupt]
unsafe fn TIMER_IRQ_2() {
    DRIVER.check_alarm(2)
}

#[interrupt]
unsafe fn TIMER_IRQ_3() {
    DRIVER.check_alarm(3)
}
