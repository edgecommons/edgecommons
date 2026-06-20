//! Record model + on-disk frame encoding (normative format, see
//! `docs/TELEMETRY_STREAMING_PHASE1.md` §3.1).
//!
//! Frame (little-endian): `[u32 frame_len][u32 crc32c][u64 offset][u64 ts_ms][u16 pk_len][pk][payload]`
//! - `frame_len` = bytes after the `frame_len` field (crc..payload).
//! - `crc32c` covers `offset..payload` (everything after the crc field).

/// A record appended by a producer. `payload` is opaque bytes; `partition_key` drives
/// ordering + downstream sharding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub partition_key: String,
    pub timestamp_ms: u64,
    pub payload: Vec<u8>,
}

impl Record {
    pub fn new(partition_key: impl Into<String>, timestamp_ms: u64, payload: impl Into<Vec<u8>>) -> Self {
        Self { partition_key: partition_key.into(), timestamp_ms, payload: payload.into() }
    }
}

const LEN: usize = 4; // frame_len
const CRC: usize = 4;
const OFF: usize = 8;
const TS: usize = 8;
const PKLEN: usize = 2;
/// Fixed bytes per frame on disk (everything but pk + payload).
pub const FRAME_OVERHEAD: usize = LEN + CRC + OFF + TS + PKLEN; // 26

/// Total on-disk size of a frame with the given key/payload lengths.
pub fn frame_size(pk_len: usize, payload_len: usize) -> usize {
    FRAME_OVERHEAD + pk_len + payload_len
}

/// Append an encoded frame for one record to `out`.
pub fn encode_frame(offset: u64, ts_ms: u64, pk: &[u8], payload: &[u8], out: &mut Vec<u8>) {
    debug_assert!(pk.len() <= u16::MAX as usize);
    let frame_len = (CRC + OFF + TS + PKLEN + pk.len() + payload.len()) as u32;

    out.extend_from_slice(&frame_len.to_le_bytes());
    let crc_pos = out.len();
    out.extend_from_slice(&0u32.to_le_bytes()); // crc placeholder
    let body_start = out.len();
    out.extend_from_slice(&offset.to_le_bytes());
    out.extend_from_slice(&ts_ms.to_le_bytes());
    out.extend_from_slice(&(pk.len() as u16).to_le_bytes());
    out.extend_from_slice(pk);
    out.extend_from_slice(payload);

    let crc = crc32c::crc32c(&out[body_start..]);
    out[crc_pos..crc_pos + CRC].copy_from_slice(&crc.to_le_bytes());
}

/// A decoded frame borrowing from the input buffer.
#[derive(Debug)]
pub struct FrameView<'a> {
    pub offset: u64,
    pub ts_ms: u64,
    pub partition_key: &'a [u8],
    pub payload: &'a [u8],
    /// Total bytes consumed from the buffer for this frame.
    pub consumed: usize,
}

/// Outcome of decoding a frame at the start of `buf`.
#[derive(Debug)]
pub enum Decoded<'a> {
    /// A complete, CRC-valid frame.
    Complete(FrameView<'a>),
    /// Not enough bytes yet (truncated tail) — stop here.
    Incomplete,
    /// A CRC or structural mismatch — stop here (torn/corrupt tail).
    Corrupt,
}

/// Decode the frame at the start of `buf`.
pub fn decode_frame(buf: &[u8]) -> Decoded<'_> {
    if buf.len() < LEN {
        return Decoded::Incomplete;
    }
    let frame_len = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    let total = LEN + frame_len;
    if frame_len < CRC + OFF + TS + PKLEN || buf.len() < total {
        return Decoded::Incomplete;
    }
    let stored_crc = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
    let body = &buf[LEN + CRC..total];
    if crc32c::crc32c(body) != stored_crc {
        return Decoded::Corrupt;
    }
    let offset = u64::from_le_bytes(body[0..8].try_into().unwrap());
    let ts_ms = u64::from_le_bytes(body[8..16].try_into().unwrap());
    let pk_len = u16::from_le_bytes([body[16], body[17]]) as usize;
    if body.len() < OFF + TS + PKLEN + pk_len {
        return Decoded::Corrupt;
    }
    let pk_start = OFF + TS + PKLEN;
    let partition_key = &body[pk_start..pk_start + pk_len];
    let payload = &body[pk_start + pk_len..];
    Decoded::Complete(FrameView { offset, ts_ms, partition_key, payload, consumed: total })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let mut buf = Vec::new();
        encode_frame(42, 1234, b"asset/temp", b"payload-bytes", &mut buf);
        assert_eq!(buf.len(), frame_size(10, 13));
        match decode_frame(&buf) {
            Decoded::Complete(f) => {
                assert_eq!(f.offset, 42);
                assert_eq!(f.ts_ms, 1234);
                assert_eq!(f.partition_key, b"asset/temp");
                assert_eq!(f.payload, b"payload-bytes");
                assert_eq!(f.consumed, buf.len());
            }
            other => panic!("expected Complete, got {other:?}"),
        }
    }

    #[test]
    fn empty_pk_and_payload() {
        let mut buf = Vec::new();
        encode_frame(0, 0, b"", b"", &mut buf);
        match decode_frame(&buf) {
            Decoded::Complete(f) => {
                assert!(f.partition_key.is_empty());
                assert!(f.payload.is_empty());
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn truncated_is_incomplete() {
        let mut buf = Vec::new();
        encode_frame(1, 1, b"k", b"vvvv", &mut buf);
        for cut in 0..buf.len() {
            assert!(matches!(decode_frame(&buf[..cut]), Decoded::Incomplete), "cut={cut}");
        }
    }

    #[test]
    fn corrupted_crc_is_detected() {
        let mut buf = Vec::new();
        encode_frame(7, 7, b"k", b"data", &mut buf);
        let last = buf.len() - 1;
        buf[last] ^= 0xFF; // flip a payload byte
        assert!(matches!(decode_frame(&buf), Decoded::Corrupt));
    }
}
