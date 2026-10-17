use core::sync::atomic::AtomicU32;

use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use embassy_time::Instant;
use embedded_io_async::Write;
use esp_hal::{
    Async,
    uart::{UartRx, UartTx},
};
use protocol::{FromDevice, ToDevice};

pub const MAX_FRAME: usize = 1024;
pub static OUTGOING: Channel<CriticalSectionRawMutex, FromDevice, 16> = Channel::new();
pub static INCOMING: Channel<CriticalSectionRawMutex, ToDevice, 16> = Channel::new();
pub static DROPPED: AtomicU32 = AtomicU32::new(0);
pub static INCOMING_FAILS: AtomicU32 = AtomicU32::new(0);

pub struct Sender;
impl Sender {
    pub async fn guarantee_send(&self, data: FromDevice) {
        OUTGOING.send(data).await
    }
    pub fn try_send(&self, data: FromDevice) {
        match OUTGOING.try_send(data) {
            Ok(_) => (),
            Err(_x) => {
                DROPPED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            }
        }
    }
}

#[embassy_executor::task]
pub async fn send_uart(mut tx: UartTx<'static, Async>) -> ! {
    let mut encoder = Encoder::new();
    loop {
        let message = OUTGOING.receive().await;
        let raw = encoder.encode(&message);
        // there are curently no tx error types so just ignore
        let _ = tx.write_all(raw).await;
    }
}
#[embassy_executor::task]
pub async fn receive_uart(mut rx: UartRx<'static, Async>) -> ! {
    let mut rx_buf = [0u8; MAX_FRAME];
    let mut scratch = [0u8; MAX_FRAME];
    let mut final_buff = [0u8; MAX_FRAME];

    let mut encoder_bytes = 0;
    loop {
        if encoder_bytes == rx_buf.len() {
            // overflowed max frame, just start over
            encoder_bytes = 0;
        }

        let result = rx.read_async(&mut rx_buf[encoder_bytes..]).await;
        let num_read = match result {
            // set num read
            Ok(x) => x,
            Err(_) => {
                // just start over
                encoder_bytes = 0;
                INCOMING_FAILS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                continue;
            }
        };
        encoder_bytes += num_read;
        let mut start = 0;

        while let Some(this_frame_len) = rx_buf[start..encoder_bytes]
            .iter()
            .position(|&x| x == b'\n')
        {
            if this_frame_len <= final_buff.len() {
                final_buff[..this_frame_len]
                    .copy_from_slice(&rx_buf[start..start + this_frame_len]);
                match decode(&mut final_buff[..this_frame_len], &mut scratch) {
                    Some(x) => {
                        INCOMING.try_send(x).unwrap();
                    }
                    None => {
                        INCOMING_FAILS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    }
                }
            }
            start = start + this_frame_len + 1;
        }
        rx_buf.copy_within(start..encoder_bytes, 0);
        encoder_bytes -= start;
    }
}

pub struct Encoder {
    buf: [u8; MAX_FRAME],
}

impl Encoder {
    fn new() -> Self {
        Self {
            buf: [0u8; MAX_FRAME],
        }
    }
    fn encode(&mut self, data: &protocol::FromDevice) -> &[u8] {
        match serde_json_core::to_slice(&data, &mut self.buf[..MAX_FRAME - 1]) {
            Ok(n) => {
                self.buf[n] = b'\n';
                &self.buf[..=n]
            }
            Err(_e) => {
                serde_json_core::to_slice(
                    &protocol::FromDevice::Log {
                        time_ms: Instant::now().as_millis(),
                        level: log::Level::Warn,
                        message: "frame too long"
                            .try_into()
                            .expect("why would this fail omg"),
                    },
                    &mut self.buf,
                )
                .unwrap(); // is compile time so should be guaranteed to fit
                &self.buf
            }
        }
    }
}
fn decode(new_data: &mut [u8], scratch: &mut [u8]) -> Option<protocol::ToDevice> {
    let result: Result<(ToDevice, _), _> = serde_json_core::from_slice_escaped(new_data, scratch);

    match result {
        Ok(v) => Some(v.0),
        Err(_e) => None,
    }
}
