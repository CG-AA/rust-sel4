//
// Copyright 2026, Slime OS contributors
//
// SPDX-License-Identifier: BSD-2-Clause
//

use spin::lock_api::Mutex;

use sel4_config::sel4_cfg_bool;

use crate::{arch::reset_cntvoff, plat::Plat};

// The Radxa Dragon Q6A's debug UART: `serial@994000`, a GENI serial engine
// (`compatible = "qcom,geni-debug-uart"`), which `tools/dts/dragon-q6a.dts`
// in the kernel names as `serial0` and `overlay-dragon-q6a.dts` designates as
// this platform's `seL4,elfloader-devices` console.
const SERIAL_DEVICE_BASE_ADDR: usize = 0x0099_4000;

// Register names as in Linux `include/linux/soc/qcom/geni-se.h`.
const SE_GENI_STATUS: usize = 0x40;
const SE_UART_TX_TRANS_LEN: usize = 0x270;
const SE_GENI_M_CMD0: usize = 0x600;
const SE_GENI_M_IRQ_STATUS: usize = 0x610;
const SE_GENI_M_IRQ_CLEAR: usize = 0x618;
const SE_GENI_TX_FIFO: usize = 0x700;
const SE_GENI_TX_WATERMARK: usize = 0x80c;

const M_GENI_CMD_ACTIVE: u32 = 1 << 0;
const M_CMD_DONE: u32 = 1 << 0;
const M_TX_FIFO_WATERMARK: u32 = 1 << 30;
const UART_START_TX: u32 = 1 << 27;
const DEFAULT_TX_WATERMARK: u32 = 2;

// A console that never becomes ready must not stop the boot: each wait gives
// up after this many reads of its status register and the byte is dropped.
const POLL_LIMIT: usize = 1 << 22;

static SERIAL_LOCK: Mutex<()> = Mutex::new(());

fn read(offset: usize) -> u32 {
    unsafe { ((SERIAL_DEVICE_BASE_ADDR + offset) as *const u32).read_volatile() }
}

fn write(offset: usize, value: u32) {
    unsafe { ((SERIAL_DEVICE_BASE_ADDR + offset) as *mut u32).write_volatile(value) }
}

fn wait(offset: usize, ready: impl Fn(u32) -> bool) -> bool {
    (0..POLL_LIMIT).any(|_| ready(read(offset)))
}

// One byte is one transmit command. The firmware leaves the engine configured
// as a UART, so no clock, pin, or protocol is set up here.
fn put_char_raw(c: u8) {
    if !wait(SE_GENI_STATUS, |status| status & M_GENI_CMD_ACTIVE == 0) {
        return;
    }
    write(SE_GENI_TX_WATERMARK, DEFAULT_TX_WATERMARK);
    write(SE_UART_TX_TRANS_LEN, 1);
    write(SE_GENI_M_CMD0, UART_START_TX);
    if !wait(SE_GENI_M_IRQ_STATUS, |status| status & M_TX_FIFO_WATERMARK != 0) {
        return;
    }
    write(SE_GENI_TX_FIFO, c.into());
    write(SE_GENI_M_IRQ_CLEAR, M_TX_FIFO_WATERMARK);
    if wait(SE_GENI_M_IRQ_STATUS, |status| status & M_CMD_DONE != 0) {
        write(SE_GENI_M_IRQ_CLEAR, M_CMD_DONE);
    }
}

pub(crate) enum PlatImpl {}

impl Plat for PlatImpl {
    fn init_per_core() {
        if sel4_cfg_bool!(ARM_HYPERVISOR_SUPPORT) {
            unsafe {
                reset_cntvoff();
            }
        }
    }

    fn put_char(c: u8) {
        let _guard = SERIAL_LOCK.lock();
        put_char_raw(c);
    }

    fn put_char_without_synchronization(c: u8) {
        put_char_raw(c);
    }

    fn start_secondary_core(core_id: usize, sp: usize) {
        // The board's firmware exposes PSCI over SMC, as the `/psci` node of
        // `tools/dts/dragon-q6a.dts` records.
        crate::arch::drivers::psci::start_secondary_core(core_id, sp)
    }
}
