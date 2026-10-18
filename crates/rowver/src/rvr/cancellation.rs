use crate::Result;
use std::io::{Error, ErrorKind, Read};

#[derive(Clone, Copy)]
pub(super) struct Cancellation<'a>(pub(super) &'a dyn Fn() -> bool);

impl Cancellation<'_> {
    /// Stop validation at a bounded operation boundary.
    pub(super) fn check(self) -> Result<()> {
        if (self.0)() {
            Err(Error::new(ErrorKind::Interrupted, "Checkpoint validation cancelled").into())
        } else {
            Ok(())
        }
    }

    /// Poll cancellation periodically while inspecting a large event array.
    pub(super) fn at(self, index: usize) -> Result<()> {
        if index.is_multiple_of(16384) {
            self.check()?;
        }
        Ok(())
    }
}

pub(super) struct CancelReader<'a, R> {
    pub(super) reader: R,
    pub(super) cancellation: Cancellation<'a>,
}

impl<R: Read> Read for CancelReader<'_, R> {
    /// Limit reads so compressed streams and header reads remain interruptible.
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if (self.cancellation.0)() {
            // Read helpers retry Interrupted; normalize this error at the public boundary.
            return Err(Error::other("Checkpoint validation cancelled"));
        }
        let size = buffer.len().min(65536);
        self.reader.read(&mut buffer[..size])
    }
}
