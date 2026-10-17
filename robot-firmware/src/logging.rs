use log::Log;

use crate::communication::OUTGOING;
extern crate alloc;

pub struct Logger {}

impl Logger {
    pub fn new() -> Self {
        Self {}
    }
}

impl Log for Logger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }

        let msg = alloc::format!("{}", record.args());
        let _ = OUTGOING.try_send(protocol::FromDevice::Log {
            time_ms: embassy_time::Instant::now().as_millis(),
            level: record.level(),
            message: msg
                .as_str()
                .try_into()
                .unwrap_or("message too long".try_into().unwrap()),
        });
    }

    fn flush(&self) {}
}
