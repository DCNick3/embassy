#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

#[path = "../example_common.rs"]
mod example_common;

use defmt::{info, unwrap};
use defmt_rtt as _; // global logger
use panic_probe as _; // print out panic messages

use embassy::executor::Spawner;
use embassy::interrupt::InterruptExt;
use embassy::io::{read_line, AsyncWriteExt};
use embassy_nrf::usb::{State, Usb, UsbBus, UsbSerial};
use embassy_nrf::{interrupt, Peripherals};
use futures::pin_mut;
use usb_device::device::{UsbDeviceBuilder, UsbVidPid};

#[embassy::main]
async fn main(_spawner: Spawner, p: Peripherals) {
    let mut rx_buffer = [0u8; 64];
    // we send back input + cr + lf
    let mut tx_buffer = [0u8; 66];

    let usb_bus = UsbBus::new(p.USBD);

    let serial = UsbSerial::new(&usb_bus, &mut rx_buffer, &mut tx_buffer);

    let device = UsbDeviceBuilder::new(&usb_bus, UsbVidPid(0x16c0, 0x27dd))
        .manufacturer("Fake company")
        .product("Serial port")
        .serial_number("TEST")
        .device_class(0x02)
        .build();

    let irq = interrupt::take!(USBD);
    irq.set_priority(interrupt::Priority::P3);

    let mut state = State::new();
    let usb = unsafe { Usb::new(&mut state, device, serial, irq) };
    pin_mut!(usb);

    let (mut reader, mut writer) = usb.as_ref().take_serial_0();

    info!("usb initialized!");

    unwrap!(
        writer
            .write_all(b"\r\nInput returned upper cased on CR+LF\r\n")
            .await
    );

    let mut buf = [0u8; 64];
    loop {
        let n = unwrap!(read_line(&mut reader, &mut buf).await);

        for char in buf[..n].iter_mut() {
            // upper case
            if 0x61 <= *char && *char <= 0x7a {
                *char &= !0x20;
            }
        }

        unwrap!(writer.write_all(&buf[..n]).await);
        unwrap!(writer.write_all(b"\r\n").await);
        unwrap!(writer.flush().await);
    }
}
