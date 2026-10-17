#![no_std]
#![allow(unused)]
use serde::Deserialize;
use serde::Serialize;

#[derive(Serialize, Deserialize, Debug)]
pub struct ToDevice {
    pub id: u32,
    pub command: Command,
}
#[derive(Serialize, Deserialize, Debug)]
pub enum Command {
    Ping,
    SetLed { brightness: u8 },
}
#[derive(Serialize, Deserialize, Debug)]
pub enum FromDevice {
    Log {
        time_ms: u64,
        level: log::Level,
        message: heapless::String<128>,
    },
    Telemetry {
        time_ms: u64,
        data: SensorData,
    },
    Response {
        id: u32,
        respones: Response,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum Response {
    Success,
    SucccessWith(heapless::String<128>),
    Failure,
    FailureWith(heapless::String<128>),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum SensorData {
    DistanceSensor {
        distance_mm: u16,
    },
    GyroScope {
        ax: f32,
        ay: f32,
        az: f32,
        gx: f32,
        gy: f32,
        gz: f32,
        temperature: f32,
    },
}
