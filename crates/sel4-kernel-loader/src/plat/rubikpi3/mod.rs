//
// Copyright 2026, Slime OS contributors
//
// SPDX-License-Identifier: BSD-2-Clause
//

use spin::Mutex;

use crate::{arch::reset_cntvoff, plat::Plat};

const SERIAL_DEVICE_BASE_ADDR: usize = 0x0099_4000;
const SE_UART_TX_TRANS_LEN: usize = 0x270;
const SE_GENI_M_CMD0: usize = 0x600;
const SE_GENI_M_IRQ_STATUS: usize = 0x610;
const SE_GENI_M_IRQ_CLEAR: usize = 0x618;
const SE_GENI_TX_FIFO: usize = 0x700;
const SE_GENI_TX_WATERMARK: usize = 0x80c;

const M_CMD_DONE: u32 = 1 << 0;
const M_TX_FIFO_WATERMARK: u32 = 1 << 30;
const UART_START_TX: u32 = 1 << 27;
const DEFAULT_TX_WATERMARK: u32 = 2;
const POLL_LIMIT: usize = 1 << 24;

static SERIAL_LOCK: Mutex<()> = Mutex::new(());

fn register(offset: usize) -> *mut u32 {
    (SERIAL_DEVICE_BASE_ADDR + offset) as *mut u32
}

fn read(offset: usize) -> u32 {
    unsafe { register(offset).read_volatile() }
}

fn write(offset: usize, value: u32) {
    unsafe { register(offset).write_volatile(value) }
}

fn wait_for_irq(mask: u32) -> bool {
    (0..POLL_LIMIT).any(|_| read(SE_GENI_M_IRQ_STATUS) & mask != 0)
}

fn put_char_raw(c: u8) {
    write(SE_GENI_TX_WATERMARK, DEFAULT_TX_WATERMARK);
    write(SE_UART_TX_TRANS_LEN, 1);
    write(SE_GENI_M_CMD0, UART_START_TX);

    if !wait_for_irq(M_TX_FIFO_WATERMARK) {
        return;
    }

    write(SE_GENI_TX_FIFO, c.into());
    write(SE_GENI_M_IRQ_CLEAR, M_TX_FIFO_WATERMARK);

    if wait_for_irq(M_CMD_DONE) {
        write(SE_GENI_M_IRQ_CLEAR, M_CMD_DONE);
    }
}

pub(crate) enum PlatImpl {}

impl Plat for PlatImpl {
    fn init_per_core() {
        unsafe {
            reset_cntvoff();
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
        crate::arch::drivers::psci::start_secondary_core(core_id, sp)
    }
}
