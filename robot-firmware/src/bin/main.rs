#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use embassy_embedded_hal::shared_bus::asynch::i2c::I2cDevice;

use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_time::Instant;
use embedded_hal::pwm::SetDutyCycle;
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::gpio::Output;
use esp_hal::gpio::OutputConfig;
use esp_hal::i2c::master::Config;
use esp_hal::i2c::master::I2c;
use esp_hal::ledc;
use esp_hal::ledc::LowSpeed;
use esp_hal::ledc::channel;
use esp_hal::ledc::channel::ChannelIFace;
use esp_hal::ledc::timer;
use esp_hal::ledc::timer::TimerIFace;
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::uart;
use esp_hal::uart::Uart;
use log::LevelFilter;
use log::info;
use mpu6050_async_driver::{Address, I2cBus};
use protocol::FromDevice;
use robot_firmware::communication::INCOMING;
use robot_firmware::communication::Sender;
use robot_firmware::communication::receive_uart;
use robot_firmware::communication::send_uart;
use robot_firmware::logging::Logger;
use robot_firmware::vl53l1x::{self, DistanceMode, NoXshut};

extern crate alloc;

macro_rules! mk_static {
    ($t:ty, $val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        #[deny(unused_attributes)]
        let x = STATIC_CELL.uninit().write($val);
        x
    }};
}

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
    init(LevelFilter::Info);

    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 98768);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    info!("Embassy initialized!");

    let _wifi_controller =
        esp_radio::wifi::WifiController::new(peripherals.WIFI, Default::default())
            .expect("Failed to initialize Wi-Fi controller");
    let _wifi_interface = esp_radio::wifi::Interface::station();

    let (rx, tx) = Uart::new(
        peripherals.UART0,
        uart::Config::default().with_baudrate(112500),
    )
    .unwrap()
    .with_tx(peripherals.GPIO1)
    .with_rx(peripherals.GPIO3)
    .into_async()
    .split();

    let i2c = I2c::new(
        peripherals.I2C0,
        Config::default().with_frequency(Rate::from_khz(400)),
    )
    .expect("Failed to init i2c")
    .with_sda(peripherals.GPIO21)
    .with_scl(peripherals.GPIO22)
    .into_async();

    let i2c_bus: Mutex<NoopRawMutex, _> = Mutex::new(i2c);

    let led = Output::new(
        peripherals.GPIO2,
        esp_hal::gpio::Level::High,
        OutputConfig::default(),
    );
    let ledc = mk_static!(ledc::Ledc<'static>, ledc::Ledc::new(peripherals.LEDC));

    ledc.set_global_slow_clock(esp_hal::ledc::LSGlobalClkSource::APBClk);

    let slow_timer = mk_static!(
        timer::Timer<'static, LowSpeed>,
        ledc.timer::<LowSpeed>(timer::Number::Timer0)
    );
    slow_timer
        .configure(timer::config::Config {
            duty: timer::config::Duty::Duty8Bit,
            frequency: Rate::from_khz(24),
            clock_source: timer::LSClockSource::APBClk,
        })
        .unwrap();
    let slow_timer: &'static timer::Timer<'static, LowSpeed> = slow_timer;

    let mut channel = ledc.channel(channel::Number::Channel0, led);
    channel
        .configure(ledc::channel::config::Config {
            timer: slow_timer,
            duty_pct: 10,
            drive_mode: esp_hal::gpio::DriveMode::PushPull,
        })
        .unwrap();

    // TODO: Spawn some tasks
    spawner.spawn(receive_uart(rx).unwrap());
    spawner.spawn(send_uart(tx).unwrap());
    spawner.spawn(recieve_messages(channel).unwrap());

    let distance_bus = &mut I2cDevice::new(&i2c_bus);
    let mut distance_sensor = vl53l1x::Vl53l1x::new(distance_bus, NoXshut);
    let bus = I2cBus::new(I2cDevice::new(&i2c_bus), Some(Address::PRIMARY));
    let mut imu = mpu6050_async_driver::Mpu6050Async::new(bus);
    imu.setup().await.unwrap();
    if !imu.is_connected().await.unwrap() {
        log::error!("sensor failed to connect");
        panic!();
    }

    distance_sensor
        .init(true, DistanceMode::Short, 20_000)
        .await
        .unwrap();
    distance_sensor.start_continuous(50).await.unwrap();

    let sender = Sender {};
    loop {
        let now = Instant::now();
        match imu.get_motion().await {
            Ok(m) => {
                let (ax, ay, az) = m.accel;
                let (gx, gy, gz) = m.gyro;
                let t = (m.temperature_celsius) * (9.0 / 5.0) + 32.0;
                sender.try_send(FromDevice::Telemetry {
                    data: protocol::SensorData::GyroScope {
                        ax,
                        ay,
                        az,
                        gx,
                        gy,
                        gz,
                        temperature: t,
                    },
                    time_ms: now.as_millis(),
                });
            }
            Err(_) => { /* I2C error */ }
        }

        let distance = distance_sensor.read().await.unwrap();
        sender.try_send(FromDevice::Telemetry {
            data: protocol::SensorData::DistanceSensor {
                distance_mm: distance.range_mm,
            },
            time_ms: now.as_millis(),
        });
    }

    // for inspiration have a look at the examples at https://github.com/esp-rs/esp-hal/tree/esp-hal-v1.2.2/examples
}

#[allow(
    clippy::large_stack_frames,
    reason = "protocol enums hold fixed-size buffers"
)]
#[embassy_executor::task]
async fn recieve_messages(mut led: ledc::channel::Channel<'static, LowSpeed>) -> ! {
    let sender = Sender;
    loop {
        let message = INCOMING.receive().await;
        match message.command {
            protocol::Command::SetLed { brightness } => {
                let err = led.set_duty_cycle_fraction(brightness as u16, 255);
                let is_successful = match err {
                    Ok(_) => protocol::Response::Success,
                    Err(_) => protocol::Response::Failure,
                };
                sender
                    .guarantee_send(FromDevice::Response {
                        id: message.id,
                        respones: is_successful,
                    })
                    .await;
            }
            protocol::Command::Ping => {
                sender
                    .guarantee_send(FromDevice::Response {
                        id: message.id,
                        respones: protocol::Response::Success,
                    })
                    .await
            }
        }
    }
}
static LOGGER: Logger = Logger {};

pub fn init(level: LevelFilter) {
    log::set_logger(&LOGGER).expect("a logger was already installed");
    log::set_max_level(level);
}
