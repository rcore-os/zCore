#![cfg(any(test, feature = "std"))]

use std::fs::File;
use std::io::{Error, Read, Seek, SeekFrom, Write};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use super::*;

impl Device for Mutex<File> {
    fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<usize> {
        let offset = offset as u64;
        let mut file = self.lock().unwrap();
        file.seek(SeekFrom::Start(offset))?;
        let len = file.read(buf)?;
        Ok(len)
    }

    fn write_at(&self, offset: usize, buf: &[u8]) -> Result<usize> {
        let offset = offset as u64;
        let mut file = self.lock().unwrap();
        file.seek(SeekFrom::Start(offset))?;
        let len = file.write(buf)?;
        Ok(len)
    }

    fn sync(&self) -> Result<()> {
        let file = self.lock().unwrap();
        file.sync_all()?;
        Ok(())
    }
}

pub struct StdTimeProvider;

impl TimeProvider for StdTimeProvider {
    fn current_time(&self) -> Timespec {
        let duration = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        Timespec {
            sec: duration.as_secs() as i64,
            nsec: duration.subsec_nanos() as i32,
        }
    }
}

impl From<Error> for DevError {
    fn from(_: Error) -> Self {
        DevError
    }
}
