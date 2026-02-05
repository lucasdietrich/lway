use std::error::Error;

pub trait Logger {
    type Error: Error;

    fn log_str(&self, name: &str, pid: i32, msg: &str) -> Result<(), Self::Error> {
        self.log(name, pid, msg.as_bytes())
    }

    fn log(&self, name: &str, pid: i32, bytes: &[u8]) -> Result<(), Self::Error>;
}

pub struct StdoutLogger;

impl Logger for StdoutLogger {
    type Error = std::io::Error;

    fn log(&self, name: &str, pid: i32, bytes: &[u8]) -> Result<(), Self::Error> {
        let string = String::from_utf8_lossy(bytes);
        for line in string.lines() {
            println!("[{}:{}] {}", name, pid, line);
        }
        Ok(())
    }
}