use std::error::Error;

pub trait Logger {
    fn log_str(&self, name: &str, pid: libc::pid_t, msg: &str) -> Result<(), Box<dyn Error>> {
        self.log(name, pid, msg.as_bytes())
    }

    fn log(&self, name: &str, pid: libc::pid_t, bytes: &[u8]) -> Result<(), Box<dyn Error>>;
}

const DEFAULT_GUTTER_WIDTH: usize = 16;

pub struct StdoutLogger {
    gutter_width: usize,
}

impl StdoutLogger {
    pub fn new(gutter_width: usize) -> Self {
        Self { gutter_width }
    }
}

impl Default for StdoutLogger {
    fn default() -> Self {
        Self {
            gutter_width: DEFAULT_GUTTER_WIDTH,
        }
    }
}

impl Logger for StdoutLogger {
    fn log(&self, name: &str, pid: libc::pid_t, bytes: &[u8]) -> Result<(), Box<dyn Error>> {
        let string = String::from_utf8_lossy(bytes);
        for line in string.lines() {
            let prefix = format!("[{} {}]", pid, name);
            println!("{:width$} {}", prefix, line, width = self.gutter_width);
        }
        Ok(())
    }
}

pub struct NoopLogger;

impl Logger for NoopLogger {
    fn log(&self, _name: &str, _pid: libc::pid_t, _bytes: &[u8]) -> Result<(), Box<dyn Error>> {
        Ok(())
    }
}
