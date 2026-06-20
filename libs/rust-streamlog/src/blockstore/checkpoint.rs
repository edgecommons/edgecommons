//! The delivery checkpoint: `[u64 acked][u64 drop_floor][u32 crc32c]`, written via a temp
//! file + atomic rename. `acked` is the **exclusive** export cursor (all offsets `< acked`
//! are delivered). `drop_floor` is the lowest retained offset (advances on dropOldest).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Checkpoint {
    /// Exclusive export cursor: all offsets `< acked` have been delivered to the sink.
    pub acked: u64,
    /// Lowest offset still retained on disk.
    pub drop_floor: u64,
}

const FILE: &str = "checkpoint";
const TMP: &str = "checkpoint.tmp";
const SIZE: usize = 8 + 8 + 4;

fn path(dir: &Path) -> PathBuf {
    dir.join(FILE)
}

/// Load the checkpoint, or the default `{0,0}` if it does not exist. A torn checkpoint
/// (bad crc / short) is treated as absent → `{0,0}` (conservative: re-deliver = at-least-once).
pub fn load(dir: &Path) -> Result<Checkpoint> {
    let p = path(dir);
    let bytes = match fs::read(&p) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Checkpoint::default()),
        Err(e) => return Err(e.into()),
    };
    if bytes.len() != SIZE {
        return Ok(Checkpoint::default());
    }
    let acked = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
    let drop_floor = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
    let crc = u32::from_le_bytes(bytes[16..20].try_into().unwrap());
    if crc32c::crc32c(&bytes[0..16]) != crc {
        return Ok(Checkpoint::default());
    }
    Ok(Checkpoint { acked, drop_floor })
}

/// Persist the checkpoint atomically (temp file + rename, fsync'd).
pub fn store(dir: &Path, cp: Checkpoint) -> Result<()> {
    let mut buf = Vec::with_capacity(SIZE);
    buf.extend_from_slice(&cp.acked.to_le_bytes());
    buf.extend_from_slice(&cp.drop_floor.to_le_bytes());
    let crc = crc32c::crc32c(&buf);
    buf.extend_from_slice(&crc.to_le_bytes());

    let tmp = dir.join(TMP);
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(&buf)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path(dir))?;
    // Best-effort directory fsync so the rename is durable (ignored where unsupported, e.g. Windows).
    if let Ok(d) = fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}
