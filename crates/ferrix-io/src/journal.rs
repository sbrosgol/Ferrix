//! An append-only, checksummed record log for crash recovery (ADR-0003 §3).
//!
//! The file starts with [`MAGIC`], followed by frames of `len: u32 LE`, `crc32: u32 LE`,
//! `payload`. Each append is flushed to disk before returning. A crash can only damage the last
//! frame, and [`read`] stops at the first frame that is incomplete or fails its checksum.
//!
//! The writer holds an exclusive OS lock on the file for as long as it lives. The lock is
//! released when the process exits for any reason, so an unlocked journal is one whose session
//! ended without cleaning up: a crash. [`is_orphaned`] checks exactly that.

use std::{
    fs::{self, File, OpenOptions, TryLockError},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

const MAGIC: &[u8; 8] = b"FRXJRNL1";
/// Larger frames can only come from corruption; refuse them rather than allocate.
const MAX_FRAME: usize = 64 * 1024 * 1024;

/// Appends records to a journal it exclusively owns.
#[derive(Debug)]
pub struct JournalWriter {
    file: File,
    path: PathBuf,
}

impl JournalWriter {
    /// Creates a new journal at `path` (which must not exist) and locks it.
    pub fn create(path: &Path) -> io::Result<Self> {
        let mut file = OpenOptions::new()
            .read(true)
            .append(true)
            .create_new(true)
            .open(path)?;
        file.try_lock().map_err(|e| match e {
            TryLockError::Error(e) => e,
            TryLockError::WouldBlock => io::Error::other("journal is locked by another process"),
        })?;
        file.write_all(MAGIC)?;
        file.sync_all()?;
        crate::sync_parent_dir(path);
        Ok(Self {
            file,
            path: path.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Appends one record and waits until it is on disk.
    pub fn append(&mut self, payload: &[u8]) -> io::Result<()> {
        if payload.len() > MAX_FRAME {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "journal record too large",
            ));
        }
        let mut frame = Vec::with_capacity(payload.len() + 8);
        frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        frame.extend_from_slice(&crc32(payload).to_le_bytes());
        frame.extend_from_slice(payload);
        self.file.write_all(&frame)?;
        self.file.sync_data()
    }

    /// Ends the session cleanly: unlocks and deletes the journal.
    pub fn discard(self) -> io::Result<()> {
        let Self { file, path } = self;
        // Close first: Windows cannot delete a file that is still open.
        drop(file);
        fs::remove_file(path)
    }
}

/// Reads every intact record, stopping at the first damaged or incomplete one.
pub fn read(path: &Path) -> io::Result<Vec<Vec<u8>>> {
    let mut bytes = Vec::new();
    File::open(path)?.read_to_end(&mut bytes)?;
    if !bytes.starts_with(MAGIC) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "not a Ferrix journal",
        ));
    }
    let mut records = Vec::new();
    let mut rest = &bytes[MAGIC.len()..];
    while rest.len() >= 8 {
        let len = u32::from_le_bytes([rest[0], rest[1], rest[2], rest[3]]) as usize;
        let crc = u32::from_le_bytes([rest[4], rest[5], rest[6], rest[7]]);
        if len > MAX_FRAME || rest.len() < 8 + len {
            break;
        }
        let payload = &rest[8..8 + len];
        if crc32(payload) != crc {
            break;
        }
        records.push(payload.to_vec());
        rest = &rest[8 + len..];
    }
    Ok(records)
}

/// True if no running session holds `path`, i.e. it was left behind by a crash.
pub fn is_orphaned(path: &Path) -> bool {
    let Ok(file) = OpenOptions::new().read(true).write(true).open(path) else {
        return false;
    };
    // The probe lock is released when `file` is dropped.
    file.try_lock().is_ok()
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = flate2::Crc::new();
    crc.update(data);
    crc.sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ferrix-journal-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("scratch dir");
        dir.join("session.journal")
    }

    #[test]
    fn records_round_trip() {
        let path = scratch("roundtrip");
        let mut journal = JournalWriter::create(&path).expect("create");
        journal.append(b"first").expect("append");
        journal.append(b"").expect("append empty");
        journal.append(&[7; 1000]).expect("append large");
        assert_eq!(
            read(&path).expect("read"),
            [b"first".to_vec(), Vec::new(), vec![7; 1000]]
        );
        journal.discard().expect("discard");
        assert!(!path.exists());
    }

    #[test]
    fn a_torn_last_record_is_ignored() {
        let path = scratch("torn");
        let mut journal = JournalWriter::create(&path).expect("create");
        journal.append(b"kept").expect("append");
        journal.append(b"cut short by a crash").expect("append");
        drop(journal);
        let len = fs::metadata(&path).expect("meta").len();
        OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("open")
            .set_len(len - 5)
            .expect("truncate");
        assert_eq!(read(&path).expect("read"), [b"kept".to_vec()]);
    }

    #[test]
    fn a_corrupted_record_ends_the_readable_prefix() {
        let path = scratch("corrupt");
        let mut journal = JournalWriter::create(&path).expect("create");
        journal.append(b"good").expect("append");
        journal.append(b"flipped").expect("append");
        journal.append(b"after").expect("append");
        drop(journal);
        let mut bytes = fs::read(&path).expect("read bytes");
        let second_payload = MAGIC.len() + 8 + 4 + 8;
        bytes[second_payload] ^= 0xFF;
        fs::write(&path, bytes).expect("write");
        assert_eq!(read(&path).expect("read"), [b"good".to_vec()]);
    }

    #[test]
    fn only_journals_without_a_live_writer_are_orphaned() {
        let path = scratch("orphan");
        let journal = JournalWriter::create(&path).expect("create");
        assert!(
            !is_orphaned(&path),
            "a live session's journal is not orphaned"
        );
        drop(journal); // the process "crashed" without discarding it
        assert!(is_orphaned(&path));
        assert!(!is_orphaned(&path.with_extension("missing")));
    }

    #[test]
    fn rejects_files_that_are_not_journals() {
        let path = scratch("foreign");
        fs::write(&path, b"%PDF-1.7").expect("write");
        assert!(read(&path).is_err());
    }
}
