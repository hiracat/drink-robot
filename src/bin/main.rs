#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use drink_robot::vl53l1x::NoXshut;
use drink_robot::vl53l1x::{self, DistanceMode};
use embassy_embedded_hal::shared_bus::asynch::i2c::I2cDevice;
use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_time::Instant;
use embassy_time::{Duration, Timer};
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::gpio::Output;
use esp_hal::gpio::OutputConfig;
use esp_hal::i2c::master::Config;
use esp_hal::i2c::master::I2c;
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
use log::info;
use mpu6050_async_driver::{Address, I2cBus};

extern crate alloc;

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    // generator version: 1.4.0
    // generator parameters: -o esp32 -o esp32-wroom-32 -o unstable-hal -o alloc -o embassy -o wifi -o log -o esp-backtrace -o neovim-classic -o esp

    esp_println::logger::init_logger_from_env();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    // The following pins are used to bootstrap the chip. They are available
    // for use, but check the datasheet of the module for more information on them.
    // - GPIO0
    // - GPIO2
    // - GPIO5
    // - GPIO12
    // - GPIO15
    // These GPIO pins are in use by some feature of the module and should not be used.
    let _gpio6 = peripherals.GPIO6;
    let _gpio7 = peripherals.GPIO7;
    let _gpio8 = peripherals.GPIO8;
    let _gpio9 = peripherals.GPIO9;
    let _gpio10 = peripherals.GPIO10;
    let _gpio11 = peripherals.GPIO11;
    let _gpio16 = peripherals.GPIO16;
    let _gpio20 = peripherals.GPIO20;

    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 98768);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    info!("Embassy initialized!");

    let _wifi_controller =
        esp_radio::wifi::WifiController::new(peripherals.WIFI, Default::default())
            .expect("Failed to initialize Wi-Fi controller");
    let _wifi_interface = esp_radio::wifi::Interface::station();

    // TODO: Spawn some tasks
    let _ = spawner;

    let mut i2c = I2c::new(
        peripherals.I2C0,
        Config::default().with_frequency(Rate::from_khz(400)),
    )
    .expect("Failed to init i2c")
    .with_sda(peripherals.GPIO21)
    .with_scl(peripherals.GPIO22)
    .into_async();

    let i2c_bus: Mutex<NoopRawMutex, _> = Mutex::new(i2c);

    let mut led = Output::new(
        peripherals.GPIO2,
        esp_hal::gpio::Level::High,
        OutputConfig::default(),
    );
    let distance_bus = &mut I2cDevice::new(&i2c_bus);
    let mut distance_sensor = vl53l1x::Vl53l1x::new(distance_bus, NoXshut);
    let bus = I2cBus::new(I2cDevice::new(&i2c_bus), Some(Address::PRIMARY));
    let mut imu = mpu6050_async_driver::Mpu6050Async::new(bus);
    imu.setup().await;
    if !imu.is_connected().await.unwrap() {
        log::error!("sensor failed to connect");
        panic!();
    }

    distance_sensor
        .init(true, DistanceMode::Short, 20_000)
        .await
        .unwrap();
    distance_sensor.start_continuous(50).await.unwrap();

    let mut previous = Instant::now();

    loop {
        match imu.get_motion().await {
            Ok(m) => {
                // m.accel and m.gyro are (x, y, z) tuples, m.temperature_celsius is f32
                // accel is in m/s², gyro in degrees/second
                let (ax, ay, az) = m.accel;
                let (gx, gy, gz) = m.gyro;
                let t = (m.temperature_celsius) * (9.0 / 5.0) + 32.0;
                info!(
                    "ax: {}, ay: {}, az: {}\n gx: {}, gy: {}, gz :{}\n t: {}",
                    ax, ay, az, gx, gy, gz, t
                )
            }
            Err(_) => { /* I2C error */ }
        }

        let distance = distance_sensor.read().await.unwrap();

        let now = Instant::now();
        let elapsed = now.duration_since(previous);
        previous = now;
        let timing_budget = distance_sensor.measurement_timing_budget().await.unwrap();
        info!("{:?}", timing_budget);

        info!(
            "distance: {} mm, status: {:?}, peak: {}, ambient:{}, loop interval: {:?}",
            distance.range_mm,
            distance.status,
            distance.peak_signal_mcps,
            distance.ambient_mcps,
            elapsed.as_millis()
        );
    }

    // for inspiration have a look at the examples at https://github.com/esp-rs/esp-hal/tree/esp-hal-v1.2.2/examples
}
