use std::{
    collections::HashMap,
    io::{Read, Write},
    time::Duration,
};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use protocol::{Command, FromDevice, Response, SensorData, ToDevice};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Layout, Margin},
    style::{Color, Styled, Stylize},
    text::{Line, Text},
    widgets::{Block, Padding, Paragraph},
};
use serialport::{SerialPort, TTYPort};

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    let terminal = ratatui::init();
    let result = App::new().run(terminal);
    ratatui::restore();
    result
}

/// The main application which holds the state and logic of the application.
#[derive(Debug)]
pub struct App {
    /// Is the application running?
    running: bool,
    port: SerialInterface,

    log_level: log::LevelFilter,

    // indexed by command id
    command_history: Vec<String>,
    command: String,

    show_telemetry: bool,
    log: Vec<(log::Level, u64, String)>,
    telemetry: Vec<(u64, SensorData)>,
    command_responses: HashMap<u32, Response>,

    _next_id: u32,
}

impl App {
    /// Construct a new instance of [`App`].
    pub fn new() -> Self {
        let port = SerialInterface::new("/dev/ttyUSB0", 112_500);
        let mut log_text = String::from("");
        if port.port.is_none() {
            log_text.push_str("serial port not found\n");
        }
        Self {
            command: String::new(),
            command_history: Vec::new(),
            port,
            running: true,
            log_level: log::LevelFilter::Info,
            show_telemetry: true,
            _next_id: 0,
            command_responses: HashMap::new(),
            log: Vec::new(),
            telemetry: Vec::new(),
        }
    }

    pub fn get_next_id(&mut self) -> u32 {
        self._next_id += 1;
        self._next_id - 1
    }

    /// Run the application's main loop.
    pub fn run(mut self, mut terminal: DefaultTerminal) -> color_eyre::Result<()> {
        self.running = true;
        while self.running {
            self.handle_crossterm_events()?;
            let mut all_data = vec![];
            while let Some(data) = self.port.read() {
                all_data.push(data);
            }
            self.ingest(&all_data);
            terminal.draw(|frame| self.render(frame))?;
        }
        Ok(())
    }

    /// Renders the user interface.
    ///
    /// This is where you add new widgets. See the following resources for more information:
    ///
    /// - <https://docs.rs/ratatui/latest/ratatui/widgets/index.html>
    /// - <https://github.com/ratatui/ratatui/tree/main/ratatui-widgets/examples>
    fn render(&mut self, frame: &mut Frame) {
        let [mut logs, cli] = frame
            .area()
            .inner(ratatui::layout::Margin {
                horizontal: 1,
                vertical: 1,
            })
            .layout::<2>(&Layout::new(
                ratatui::layout::Direction::Horizontal,
                [Constraint::Ratio(2, 3), Constraint::Ratio(1, 3)],
            ));
        let main_title = Line::from("Drink TUI").bold().blue().centered();
        let quit_text = "Press `Esc`, `Ctrl-C` or `q` to stop running.";
        frame.render_widget(
            Paragraph::new(quit_text)
                .block(Block::bordered().title(main_title))
                .centered(),
            frame.area(),
        );

        let title = Line::from("Dink CLI").bold().blue().centered();
        let help_text = "Type help for help";
        frame.render_widget(
            Paragraph::new(help_text)
                .block(Block::bordered().title(title))
                .centered(),
            cli,
        );

        let mut cli_lines: Vec<Line> = vec![Line::from("")];
        for (idx, command) in self.command_history.iter().enumerate() {
            cli_lines.push(Line::from(format!("> {command}")));

            let result = match self.command_responses.get(&(idx as u32)) {
                Some(x) => match x {
                    Response::Success => "Command succeeded".green(),
                    Response::SucccessWith(string_inner) => {
                        format!("Command succeeded with \"{}\"", string_inner.as_str()).green()
                    }
                    Response::Failure => "Command failed".red(),
                    Response::FailureWith(string_inner) => {
                        format!("Commmand failed with \"{}\"", string_inner.as_str()).red()
                    }
                },
                None => "Command has not been acknowledged".yellow(),
            };
            cli_lines.push(Line::from(result));
        }
        cli_lines.push(Line::from(format!("> {}", self.command)));

        let scroll = (cli_lines.len() as u16 + 2).saturating_sub(cli.height);
        frame.render_widget(
            Paragraph::new(Text::from(cli_lines))
                .left_aligned()
                .scroll((scroll, 0))
                .block(Block::new().padding(Padding::symmetric(2, 1))),
            cli,
        );

        if self.show_telemetry {
            let [new_logs, telemetry] = logs.inner(Margin::new(0, 0)).layout::<2>(&Layout::new(
                ratatui::layout::Direction::Vertical,
                [Constraint::Ratio(2, 3), Constraint::Ratio(1, 3)],
            ));
            logs = new_logs;
            let title = Line::from("Drink ESP live telemetry")
                .bold()
                .blue()
                .centered();

            let num_lines = self.telemetry.len();
            let log_scroll = (num_lines as u16 + 2).saturating_sub(telemetry.height);
            frame.render_widget(
                Paragraph::new(
                    self.telemetry
                        .iter()
                        .map(|(time, data)| format!("{}: {:?}", time, data))
                        .collect::<Vec<String>>()
                        .join("\n")
                        .as_str(),
                )
                .block(Block::bordered().title(title))
                .left_aligned()
                .scroll((log_scroll, 0)),
                telemetry,
            )
        }
        let title = Line::from("Drink ESP logs").bold().blue().centered();
        let num_logs = self.log.len();
        let log_scroll = (num_logs as u16 + 2).saturating_sub(logs.height);
        let mut log_lines = Vec::new();
        for (level, time, message) in self.log.iter() {
            log_lines.push(Line::from(
                format!("[{}] {}: {}", level, time, message).set_style(match level {
                    log::Level::Error => Color::Red,
                    log::Level::Warn => Color::Yellow,
                    log::Level::Info => Color::Green,
                    log::Level::Debug => Color::Cyan,
                    log::Level::Trace => Color::Gray,
                }),
            ));
        }
        frame.render_widget(
            Paragraph::new(Text::from(log_lines))
                .block(Block::bordered().title(title))
                .left_aligned()
                .scroll((log_scroll, 0)),
            logs,
        )
    }
    fn ingest(&mut self, data: &[FromDevice]) {
        for msg in data {
            match msg {
                FromDevice::Log {
                    level,
                    time_ms,
                    message,
                } if *level <= self.log_level => {
                    self.log.push((*level, *time_ms, message.to_string()));
                }
                FromDevice::Telemetry { time_ms, data } => {
                    self.telemetry.push((*time_ms, data.clone()));
                }
                FromDevice::Response { id, respones } => {
                    self.command_responses.insert(*id, respones.clone());
                }
                _ => {}
            }
        }
    }

    /// Reads the crossterm events and updates the state of [`App`].
    ///
    /// If your application needs to perform work in between handling events, you can use the
    /// [`event::poll`] function to check if there are any events available with a timeout.
    fn handle_crossterm_events(&mut self) -> color_eyre::Result<()> {
        if event::poll(Duration::from_millis(30))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => self.on_key_event(key),
                _ => {}
            }
        }

        Ok(())
    }

    /// Handles the key events and updates the state of [`App`].
    fn on_key_event(&mut self, key: KeyEvent) {
        match (key.modifiers, key.code) {
            (_, KeyCode::Esc)
            | (KeyModifiers::CONTROL, KeyCode::Char('c') | KeyCode::Char('C')) => self.quit(),
            // Add other key handlers here.
            (_, KeyCode::Char(code)) => {
                self.command.push(code);
            }
            (_, KeyCode::Backspace) => {
                self.command.pop();
            }
            (_, KeyCode::Enter) => {
                let cmd = std::mem::take(&mut self.command);
                self.command_history.push(cmd.clone());
                let id = self.get_next_id();
                let result = self.dispatch_command(&cmd, id);
                match result {
                    Ok(()) => (),
                    Err(x) => {
                        self.command_responses.insert(
                            id,
                            Response::FailureWith(
                                heapless::String::try_from(truncate_to_boundary(&x, 128))
                                    .expect("should fit"),
                            ),
                        );
                    }
                };
            }
            _ => (),
        }
    }

    /// Set running to false to quit the application.
    fn quit(&mut self) {
        self.running = false;
    }

    fn dispatch_command(&mut self, command: &str, id: u32) -> Result<(), String> {
        if command == "ping" {
            self.ping(id);
            Ok(())
        } else if command.starts_with("set led") {
            let brightness = command
                .strip_prefix("set led")
                .unwrap()
                .trim()
                .parse::<u32>()
                .map_err(|x| x.to_string())?;
            self.set_led(brightness, id);
            Ok(())
        } else {
            Err(format!("invalid command: {}", command))
        }
    }
    fn dispatch_set(&mut self, command: &str) {}
    fn dispatch_send(&mut self, command: &str) {}

    fn ping(&mut self, id: u32) {
        self.port.write(ToDevice {
            id,
            command: Command::Ping,
        });
    }
    fn set_led(&mut self, brightness_percent: u32, id: u32) {
        self.port.write(protocol::ToDevice {
            id,
            command: Command::SetLed {
                brightness: ((brightness_percent as f32 / 100.0) * 255.0) as u8,
            },
        });
    }
}
impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
struct SerialInterface {
    port: Option<TTYPort>,
    last: Vec<u8>,
}

impl SerialInterface {
    fn new(path: &str, baud_rate: u32) -> Self {
        let port = serialport::new(path, baud_rate).open_native().ok();
        Self {
            port,
            last: Vec::new(),
        }
    }
    fn write(&mut self, data: ToDevice) {
        let mut raw = serde_json::to_vec(&data).expect("seralization should not fail");
        raw.push(b'\n');
        if self.port.is_none() {
            return;
        }
        let _ = self.port.as_mut().unwrap().write_all(&raw);
    }
    fn read(&mut self) -> Option<FromDevice> {
        let bytes_available = self.port.as_mut()?.bytes_to_read().ok()?;
        let mut bytes_read = 0;
        let mut result = Vec::new();
        if bytes_available != 0 {
            result = vec![0u8; bytes_available as usize];
            bytes_read = self.port.as_mut()?.read(&mut result).ok()?;
            if bytes_read == 0 {
                return None;
            }
        }

        self.last.append(&mut result[0..bytes_read].to_vec());

        let found_position = self.last.iter().position(|x| *x == b'\n')?;

        let parsed = serde_json::from_str::<FromDevice>(&String::from_utf8_lossy(
            &self.last[0..found_position],
        ));
        // skip the newline
        self.last.drain(..=found_position);

        parsed.ok()
    }
}
fn truncate_to_boundary(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}
