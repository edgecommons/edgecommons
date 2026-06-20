//! `SegmentLog` — the default Phase-1 [`BlockStore`]: a directory of append-only segment
//! files named by zero-padded base offset, plus an atomic checkpoint. Recovery scans the
//! active (last) segment, validates the tail, and truncates a torn record.

use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use super::checkpoint::{self, Checkpoint};
use super::{BlockStore, OwnedRecord, RecoveryReport};
use crate::error::{GgStreamError, Result};
use crate::record::{self, Decoded};

struct SegMeta {
    path: PathBuf,
    /// First offset held by this segment (inclusive).
    base: u64,
    byte_len: u64,
    /// Exclusive max offset held by this segment.
    end: u64,
    /// Byte offset of each record within the file, indexed by `offset - base`. Lets the export
    /// reader seek directly to a record instead of rescanning the whole segment. Built live on
    /// append (and during tail recovery) for the active segment; built lazily on first read for
    /// older segments. A cache — always rebuildable from the `.seg` file.
    index: Vec<u64>,
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
                let (end, byte_len, was_torn, index) = scan_tail(&path, base)?;
                next_offset = end;
                torn = was_torn;
                segs.push(SegMeta { path, base, byte_len, end, index });
            } else {
                // Trust non-tail segments; their range ends where the next segment begins. Their
                // index is built lazily on first read (avoids scanning every segment at open).
                let byte_len = fs::metadata(&path)?.len();
                let end = bases[i + 1];
                segs.push(SegMeta { path, base, byte_len, end, index: Vec::new() });
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

    /// Exclusive max offset of the oldest retained segment (for retention accounting).
    pub fn oldest_end(&self) -> Option<u64> {
        self.segs.first().map(|s| s.end)
    }

    /// Number of segments currently on disk.
    pub fn segment_count(&self) -> usize {
        self.segs.len()
    }

    fn start_segment(&mut self, base: u64) -> Result<()> {
        if let Some(w) = self.writer.as_mut() {
            w.flush()?;
        }
        let path = self.dir.join(seg_name(base));
        let f = OpenOptions::new().create(true).write(true).truncate(true).open(&path)?;
        self.writer = Some(BufWriter::new(f));
        self.segs.push(SegMeta { path, base, byte_len: 0, end: base, index: Vec::new() });
        Ok(())
    }

    /// Ensure `segs[i]` has its byte-offset index built (lazy for older segments), by scanning the
    /// file once and caching. No-op for an already-indexed or empty segment.
    fn ensure_index(&mut self, i: usize) -> Result<()> {
        let seg = &self.segs[i];
        if !seg.index.is_empty() || seg.end <= seg.base {
            return Ok(());
        }
        let bytes = fs::read(&seg.path)?;
        let mut index = Vec::with_capacity((seg.end - seg.base) as usize);
        let mut pos = 0usize;
        while let Decoded::Complete(f) = record::decode_frame(&bytes[pos..]) {
            index.push(pos as u64);
            pos += f.consumed;
        }
        self.segs[i].index = index;
        Ok(())
    }

    /// Plan a read of up to `max_records`/`max_bytes` from `from` (inclusive): the exact file byte
    /// ranges to read, computed from the in-memory index with **no record file I/O** (only a lazy
    /// index build for a not-yet-indexed older segment touches disk). The caller reads the returned
    /// chunks with [`read_chunks`] — off the buffer lock.
    pub(crate) fn plan_read(
        &mut self,
        from: u64,
        max_records: usize,
        max_bytes: usize,
    ) -> Result<Vec<ReadChunk>> {
        let mut chunks = Vec::new();
        if from >= self.next_offset || max_records == 0 {
            return Ok(chunks);
        }
        let start = self.segs.iter().position(|s| from < s.end).unwrap_or(self.segs.len());
        let mut total_records = 0usize;
        let mut total_bytes = 0usize;
        let mut want = from;

        for i in start..self.segs.len() {
            if total_records >= max_records || total_bytes >= max_bytes {
                break;
            }
            self.ensure_index(i)?;
            let seg = &self.segs[i];
            if want >= seg.end {
                continue;
            }
            // If `from` predates this segment's base (older data reclaimed by retention), begin at
            // the oldest surviving record rather than skipping the segment.
            let effective = want.max(seg.base);
            let local_start = (effective - seg.base) as usize;
            let nrec = seg.index.len();

            let mut take = 0usize;
            let mut take_bytes = 0usize;
            while local_start + take < nrec
                && total_records + take < max_records
                && total_bytes + take_bytes < max_bytes
            {
                let rec_start = seg.index[local_start + take];
                let rec_end =
                    seg.index.get(local_start + take + 1).copied().unwrap_or(seg.byte_len);
                take_bytes += (rec_end - rec_start) as usize;
                take += 1;
            }
            if take == 0 {
                continue;
            }

            let byte_start = seg.index[local_start];
            let byte_end = seg.index.get(local_start + take).copied().unwrap_or(seg.byte_len);
            chunks.push(ReadChunk {
                path: seg.path.clone(),
                byte_start,
                byte_len: byte_end - byte_start,
            });
            total_records += take;
            total_bytes += take_bytes;
            want = seg.base + (local_start + take) as u64;
        }
        Ok(chunks)
    }
}

/// A contiguous byte range to read from one segment file (the output of [`SegmentLog::plan_read`]).
pub(crate) struct ReadChunk {
    pub path: PathBuf,
    pub byte_start: u64,
    pub byte_len: u64,
}

/// Read + decode the planned chunks **off the buffer lock**.
///
/// Tolerant of a chunk's segment having been reclaimed by `DropOldest` between planning and reading
/// (a benign race under disk pressure): on a missing/short file it returns the records gathered so
/// far and stops — the dropped records are gone by design, and the exporter re-reads from the
/// advanced cursor on its next poll.
pub(crate) fn read_chunks(chunks: &[ReadChunk]) -> Result<Vec<OwnedRecord>> {
    let mut out = Vec::new();
    for c in chunks {
        let mut f = match File::open(&c.path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break, // reclaimed mid-read
            Err(e) => return Err(e.into()),
        };
        if f.seek(SeekFrom::Start(c.byte_start)).is_err() {
            break;
        }
        let mut buf = vec![0u8; c.byte_len as usize];
        if f.read_exact(&mut buf).is_err() {
            break; // file shorter than planned (reclaimed/truncated) — stop with what we have
        }
        let mut pos = 0usize;
        while pos < buf.len() {
            match record::decode_frame(&buf[pos..]) {
                Decoded::Complete(fr) => {
                    pos += fr.consumed;
                    out.push(OwnedRecord {
                        offset: fr.offset,
                        ts_ms: fr.ts_ms,
                        partition_key: fr.partition_key.to_vec(),
                        payload: fr.payload.to_vec(),
                    });
                }
                Decoded::Incomplete | Decoded::Corrupt => break,
            }
        }
    }
    Ok(out)
}

/// Scan a segment from its start, returning `(next_offset, valid_byte_len, torn, index)` and
/// truncating any torn/partial tail record on disk. `index[k]` is the byte offset of the record
/// at `base + k` (so the reopened active segment has a live index without a second scan).
fn scan_tail(path: &Path, base: u64) -> Result<(u64, u64, bool, Vec<u64>)> {
    let bytes = fs::read(path)?;
    let mut pos = 0usize;
    let mut expected = base;
    let mut index = Vec::new();
    // Exits on Incomplete/Corrupt (torn tail); the inner break handles an offset gap.
    while let Decoded::Complete(f) = record::decode_frame(&bytes[pos..]) {
        if f.offset != expected {
            break; // offset gap → treat as torn boundary
        }
        index.push(pos as u64);
        pos += f.consumed;
        expected += 1;
    }
    let torn = pos < bytes.len();
    if torn {
        // Truncate the partial/corrupt tail so future appends are clean.
        let f = OpenOptions::new().write(true).open(path)?;
        f.set_len(pos as u64)?;
        f.sync_all()?;
    }
    Ok((expected, pos as u64, torn, index))
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
        active.index.push(active.byte_len); // byte offset of this record within the file
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

    fn read_from(&mut self, from: u64, max_records: usize, max_bytes: usize) -> Result<Vec<OwnedRecord>> {
        // Plan (uses the in-memory index) then read the files. The plan/read split lets the upper
        // layer hold the buffer lock only for planning and do the file I/O off-lock; this trait
        // entry point does both for direct callers/tests.
        let plan = self.plan_read(from, max_records, max_bytes)?;
        read_chunks(&plan)
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
