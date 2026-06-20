//! `SegmentLog` — the default Phase-1 [`BlockStore`]: a directory of append-only segment
//! files named by zero-padded base offset, plus an atomic checkpoint. Recovery scans the
//! active (last) segment, validates the tail, and truncates a torn record.

use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use super::checkpoint::{self, Checkpoint};
use super::{BlockStore, OwnedRecord, RecoveryReport};
use crate::error::{GgStreamError, Result};
use crate::record::{self, Decoded};

struct SegMeta {
    path: PathBuf,
    byte_len: u64,
    /// Exclusive max offset held by this segment.
    end: u64,
}

/// A segmented, append-only durable log.
pub struct SegmentLog {
    dir: PathBuf,
    segment_bytes: u64,
    segs: Vec<SegMeta>,
    writer: Option<BufWriter<File>>,
    next_offset: u64,
    recovery: RecoveryReport,
}

fn seg_name(base: u64) -> String {
    format!("{base:020}.seg")
}

fn parse_base(path: &Path) -> Option<u64> {
    path.file_stem()?.to_str()?.parse::<u64>().ok()
}

impl SegmentLog {
    /// Open (creating if needed) and recover the log at `dir`.
    pub fn open(dir: impl AsRef<Path>, segment_bytes: u64) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir)?;
        // Best-effort meta (forward-compat; not validated in Phase 1).
        let meta = dir.join("meta.json");
        if !meta.exists() {
            let _ = fs::write(
                &meta,
                format!("{{\"format\":1,\"segmentBytes\":{segment_bytes}}}"),
            );
        }

        // Discover segments by filename.
        let mut bases: Vec<u64> = Vec::new();
        for entry in fs::read_dir(&dir)? {
            let p = entry?.path();
            if p.extension().and_then(|e| e.to_str()) == Some("seg") {
                if let Some(b) = parse_base(&p) {
                    bases.push(b);
                }
            }
        }
        bases.sort_unstable();

        let mut segs: Vec<SegMeta> = Vec::with_capacity(bases.len());
        let mut next_offset = 0u64;
        let mut torn = false;

        for (i, &base) in bases.iter().enumerate() {
            let path = dir.join(seg_name(base));
            let is_last = i == bases.len() - 1;
            if is_last {
                let (end, byte_len, was_torn) = scan_tail(&path, base)?;
                next_offset = end;
                torn = was_torn;
                segs.push(SegMeta { path, byte_len, end });
            } else {
                // Trust non-tail segments; their range ends where the next segment begins.
                let byte_len = fs::metadata(&path)?.len();
                let end = bases[i + 1];
                segs.push(SegMeta { path, byte_len, end });
            }
        }

        // Re-open the active segment for appending.
        let writer = match segs.last() {
            Some(active) => {
                let f = OpenOptions::new().append(true).open(&active.path)?;
                Some(BufWriter::new(f))
            }
            None => None,
        };

        let recovery = RecoveryReport { next_offset, torn_truncated: torn, segments: segs.len() };
        Ok(Self { dir, segment_bytes, segs, writer, next_offset, recovery })
    }

    /// The recovery report from [`open`].
    pub fn recovery(&self) -> RecoveryReport {
        self.recovery
    }

    fn start_segment(&mut self, base: u64) -> Result<()> {
        if let Some(w) = self.writer.as_mut() {
            w.flush()?;
        }
        let path = self.dir.join(seg_name(base));
        let f = OpenOptions::new().create(true).write(true).truncate(true).open(&path)?;
        self.writer = Some(BufWriter::new(f));
        self.segs.push(SegMeta { path, byte_len: 0, end: base });
        Ok(())
    }
}

/// Scan a segment from its start, returning `(next_offset, valid_byte_len, torn)` and
/// truncating any torn/partial tail record on disk.
fn scan_tail(path: &Path, base: u64) -> Result<(u64, u64, bool)> {
    let bytes = fs::read(path)?;
    let mut pos = 0usize;
    let mut expected = base;
    loop {
        match record::decode_frame(&bytes[pos..]) {
            Decoded::Complete(f) => {
                if f.offset != expected {
                    break; // offset gap → treat as torn boundary
                }
                pos += f.consumed;
                expected += 1;
            }
            Decoded::Incomplete | Decoded::Corrupt => break,
        }
    }
    let torn = pos < bytes.len();
    if torn {
        // Truncate the partial/corrupt tail so future appends are clean.
        let f = OpenOptions::new().write(true).open(path)?;
        f.set_len(pos as u64)?;
        f.sync_all()?;
    }
    Ok((expected, pos as u64, torn))
}

impl BlockStore for SegmentLog {
    fn next_offset(&self) -> u64 {
        self.next_offset
    }

    fn append(&mut self, offset: u64, ts_ms: u64, pk: &[u8], payload: &[u8]) -> Result<()> {
        if offset != self.next_offset {
            return Err(GgStreamError::Corrupt(format!(
                "append offset {offset} != next {}",
                self.next_offset
            )));
        }
        if pk.len() > u16::MAX as usize {
            return Err(GgStreamError::Config("partition key exceeds 65535 bytes".into()));
        }
        let size = record::frame_size(pk.len(), payload.len()) as u64;

        let need_new = match self.segs.last() {
            None => true,
            Some(active) => active.byte_len > 0 && active.byte_len + size > self.segment_bytes,
        };
        if need_new {
            self.start_segment(offset)?;
        }

        let mut buf = Vec::with_capacity(size as usize);
        record::encode_frame(offset, ts_ms, pk, payload, &mut buf);
        self.writer.as_mut().expect("active writer").write_all(&buf)?;

        let active = self.segs.last_mut().expect("active segment");
        active.byte_len += size;
        self.next_offset = offset + 1;
        active.end = self.next_offset;
        Ok(())
    }

    fn flush_os(&mut self) -> Result<()> {
        if let Some(w) = self.writer.as_mut() {
            w.flush()?;
        }
        Ok(())
    }

    fn sync(&mut self) -> Result<()> {
        self.flush_os()?;
        if let Some(w) = self.writer.as_ref() {
            w.get_ref().sync_data()?;
        }
        Ok(())
    }

    fn read_from(&self, from: u64, max_records: usize, max_bytes: usize) -> Result<Vec<OwnedRecord>> {
        let mut out = Vec::new();
        if from >= self.next_offset || max_records == 0 {
            return Ok(out);
        }
        let mut total_bytes = 0usize;
        let start = self.segs.iter().position(|s| from < s.end).unwrap_or(self.segs.len());
        'segs: for seg in &self.segs[start..] {
            let bytes = fs::read(&seg.path)?;
            let mut pos = 0usize;
            while pos < bytes.len() {
                match record::decode_frame(&bytes[pos..]) {
                    Decoded::Complete(f) => {
                        pos += f.consumed;
                        if f.offset < from {
                            continue;
                        }
                        total_bytes += f.payload.len();
                        out.push(OwnedRecord {
                            offset: f.offset,
                            ts_ms: f.ts_ms,
                            partition_key: f.partition_key.to_vec(),
                            payload: f.payload.to_vec(),
                        });
                        if out.len() >= max_records || total_bytes >= max_bytes {
                            break 'segs;
                        }
                    }
                    Decoded::Incomplete | Decoded::Corrupt => continue 'segs,
                }
            }
        }
        Ok(out)
    }

    fn truncate_below(&mut self, offset: u64) -> Result<u64> {
        let mut reclaimed = 0u64;
        while self.segs.len() > 1 && self.segs[0].end <= offset {
            let seg = self.segs.remove(0);
            reclaimed += seg.byte_len;
            fs::remove_file(&seg.path)?;
        }
        Ok(reclaimed)
    }

    fn load_checkpoint(&self) -> Result<Checkpoint> {
        checkpoint::load(&self.dir)
    }

    fn store_checkpoint(&mut self, cp: Checkpoint) -> Result<()> {
        checkpoint::store(&self.dir, cp)
    }

    fn disk_bytes(&self) -> u64 {
        self.segs.iter().map(|s| s.byte_len).sum()
    }

    fn oldest_ts_ms(&self) -> Option<u64> {
        let seg = self.segs.first()?;
        let bytes = fs::read(&seg.path).ok()?;
        match record::decode_frame(&bytes) {
            Decoded::Complete(f) => Some(f.ts_ms),
            _ => None,
        }
    }
}
