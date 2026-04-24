// Source: CMRemote, clean-room implementation.

//! SignalR hub-protocol record framing.
//!
//! Re-derived independently from `docs/wire-protocol.md` ➜
//! *Transport* and from the public SignalR hub-protocol spec
//! referenced therein. Two encodings are pinned:
//!
//! * **JSON**: each record is a UTF-8 JSON value terminated by the
//!   ASCII record-separator byte `0x1E`. A WebSocket text frame may
//!   carry one or many concatenated records.
//! * **MessagePack**: each record is preceded by a SignalR-style
//!   [varint](https://learn.microsoft.com/aspnet/core/signalr/messagepackhubprotocol)
//!   length prefix (7 bits per byte, MSB = continuation bit, little-endian),
//!   followed by exactly that many MessagePack-encoded bytes.
//!
//! Mixing encodings on a single connection is a protocol violation.
//! The framers in this module are therefore single-encoding by
//! construction — pick one at handshake time and use the same
//! splitter / writer for the lifetime of the connection.

use std::collections::VecDeque;

/// ASCII record-separator byte that terminates every JSON record.
pub const RECORD_SEPARATOR: u8 = 0x1E;

/// Maximum number of bytes a varint length prefix may consume.
///
/// SignalR pins the prefix at 5 bytes (sufficient for a 35-bit
/// length, far above any realistic record size). We mirror that
/// upper bound; a longer prefix is rejected as malformed rather
/// than allowed to grow without bound.
pub const VARINT_MAX_BYTES: usize = 5;

/// Maximum length of a single record we are willing to buffer
/// before tearing down the connection.
///
/// Records larger than this are almost certainly an attempt to
/// exhaust agent memory: legitimate SignalR records (heartbeats,
/// invocations, completions) are kilobyte-scale. The cap is
/// generous enough for the largest real payloads we anticipate
/// (chunked script output) but small enough that it cannot DoS the
/// agent's RSS.
pub const MAX_RECORD_BYTES: usize = 16 * 1024 * 1024;

/// Errors raised by the framers.
#[derive(Debug, thiserror::Error)]
pub enum FramingError {
    /// A varint length prefix overflowed [`VARINT_MAX_BYTES`].
    #[error("varint length prefix exceeds {VARINT_MAX_BYTES} bytes")]
    VarintTooLong,

    /// A record's declared length exceeds [`MAX_RECORD_BYTES`].
    #[error("record length {0} exceeds maximum of {MAX_RECORD_BYTES} bytes")]
    RecordTooLarge(usize),
}

/// Streaming splitter for newline-record-separator-framed JSON.
///
/// Push raw bytes from the WebSocket as they arrive via
/// [`JsonFrameReader::push`]; pop fully-framed records via
/// [`JsonFrameReader::next_record`]. Partial records remain in the
/// internal buffer until the next push completes them.
#[derive(Debug, Default)]
pub struct JsonFrameReader {
    buf: Vec<u8>,
    /// Records already split out, ready to hand to the caller.
    ready: VecDeque<Vec<u8>>,
}

impl JsonFrameReader {
    /// Create an empty reader.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append bytes received from the wire and split out any
    /// complete records they unlock.
    ///
    /// Empty records (two consecutive separators) are silently
    /// dropped; the SignalR JSON spec does not assign them any
    /// meaning and forwarding them would only confuse the upstream
    /// dispatch layer.
    pub fn push(&mut self, bytes: &[u8]) -> Result<(), FramingError> {
        self.buf.extend_from_slice(bytes);

        let mut start = 0usize;
        for (i, &b) in self.buf.iter().enumerate() {
            if b == RECORD_SEPARATOR {
                let record = &self.buf[start..i];
                if record.len() > MAX_RECORD_BYTES {
                    return Err(FramingError::RecordTooLarge(record.len()));
                }
                if !record.is_empty() {
                    self.ready.push_back(record.to_vec());
                }
                start = i + 1;
            }
        }

        // Any trailing partial record stays in the buffer for the
        // next push call. The drain is cheap because we keep the
        // tail in the same allocation.
        if start > 0 {
            self.buf.drain(..start);
        } else if self.buf.len() > MAX_RECORD_BYTES {
            // Unterminated record larger than the cap: bail rather
            // than buffer indefinitely.
            let len = self.buf.len();
            self.buf.clear();
            return Err(FramingError::RecordTooLarge(len));
        }

        Ok(())
    }

    /// Pop the next fully-framed record, if any.
    pub fn next_record(&mut self) -> Option<Vec<u8>> {
        self.ready.pop_front()
    }

    /// Number of bytes currently buffered (partial record).
    pub fn buffered(&self) -> usize {
        self.buf.len()
    }
}

/// Frame a single JSON record for the wire by appending the
/// record-separator byte.
///
/// The returned `Vec<u8>` is suitable for direct WebSocket text-frame
/// transmission. Allocating per call is intentional: at our
/// throughput envelope the simplicity is worth more than micro-tuning
/// the allocator behaviour.
pub fn write_json_record(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 1);
    out.extend_from_slice(payload);
    out.push(RECORD_SEPARATOR);
    out
}

/// Streaming reader for varint-length-prefixed MessagePack records.
///
/// Same shape as [`JsonFrameReader`]: push wire bytes, pop records.
/// MessagePack records are binary and can legitimately contain
/// `0x1E`, so the JSON splitter is not interchangeable.
#[derive(Debug, Default)]
pub struct MsgPackFrameReader {
    buf: Vec<u8>,
    ready: VecDeque<Vec<u8>>,
}

impl MsgPackFrameReader {
    /// Create an empty reader.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append bytes from the wire and split out completed records.
    pub fn push(&mut self, bytes: &[u8]) -> Result<(), FramingError> {
        self.buf.extend_from_slice(bytes);

        loop {
            match decode_varint(&self.buf)? {
                None => break, // not enough bytes for the prefix yet
                Some((len, prefix_bytes)) => {
                    if len > MAX_RECORD_BYTES {
                        return Err(FramingError::RecordTooLarge(len));
                    }
                    if self.buf.len() < prefix_bytes + len {
                        // prefix complete but body still arriving
                        break;
                    }
                    let start = prefix_bytes;
                    let end = prefix_bytes + len;
                    let record = self.buf[start..end].to_vec();
                    self.ready.push_back(record);
                    self.buf.drain(..end);
                }
            }
        }

        Ok(())
    }

    /// Pop the next fully-framed record, if any.
    pub fn next_record(&mut self) -> Option<Vec<u8>> {
        self.ready.pop_front()
    }

    /// Number of bytes currently buffered (partial record).
    pub fn buffered(&self) -> usize {
        self.buf.len()
    }
}

/// Frame a single MessagePack record for the wire by prepending
/// the SignalR-style varint length prefix.
pub fn write_msgpack_record(payload: &[u8]) -> Result<Vec<u8>, FramingError> {
    if payload.len() > MAX_RECORD_BYTES {
        return Err(FramingError::RecordTooLarge(payload.len()));
    }
    let mut out = Vec::with_capacity(payload.len() + VARINT_MAX_BYTES);
    encode_varint(payload.len(), &mut out);
    out.extend_from_slice(payload);
    Ok(out)
}

/// Encode `value` as a SignalR varint into `out`.
///
/// Each byte holds 7 bits of value; the high bit is the
/// continuation flag (1 = "more bytes follow", 0 = "last byte").
/// The sequence is little-endian.
fn encode_varint(mut value: usize, out: &mut Vec<u8>) {
    loop {
        let byte = (value & 0x7F) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// Try to decode a varint from the head of `buf`.
///
/// Returns `Ok(None)` if `buf` is too short for the prefix to be
/// complete (continuation bit still set on the last byte).
/// Returns `Ok(Some((value, bytes_consumed)))` on success and
/// `Err(VarintTooLong)` if the prefix exceeds [`VARINT_MAX_BYTES`].
fn decode_varint(buf: &[u8]) -> Result<Option<(usize, usize)>, FramingError> {
    let mut value: u64 = 0;
    let mut shift = 0u32;
    for (i, &b) in buf.iter().enumerate() {
        if i >= VARINT_MAX_BYTES {
            return Err(FramingError::VarintTooLong);
        }
        // 7 bits per byte; shift never reaches 64 because of the
        // VARINT_MAX_BYTES cap above (5 * 7 = 35).
        value |= u64::from(b & 0x7F) << shift;
        if b & 0x80 == 0 {
            // usize on 32-bit targets cannot fit a 35-bit value;
            // the MAX_RECORD_BYTES cap catches that case before we
            // ever build a record that big.
            let value = usize::try_from(value).map_err(|_| FramingError::RecordTooLarge(0))?;
            return Ok(Some((value, i + 1)));
        }
        shift += 7;
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- JSON framing ----------------------------------------------------

    #[test]
    fn json_writer_appends_separator_byte() {
        let out = write_json_record(b"{\"type\":6}");
        assert_eq!(out.last(), Some(&RECORD_SEPARATOR));
        assert_eq!(&out[..out.len() - 1], b"{\"type\":6}");
    }

    #[test]
    fn json_reader_splits_concatenated_records() {
        let mut r = JsonFrameReader::new();
        let mut wire = Vec::new();
        wire.extend(write_json_record(b"{\"a\":1}"));
        wire.extend(write_json_record(b"{\"b\":2}"));
        wire.extend(write_json_record(b"{\"c\":3}"));
        r.push(&wire).unwrap();
        assert_eq!(r.next_record().as_deref(), Some(&b"{\"a\":1}"[..]));
        assert_eq!(r.next_record().as_deref(), Some(&b"{\"b\":2}"[..]));
        assert_eq!(r.next_record().as_deref(), Some(&b"{\"c\":3}"[..]));
        assert!(r.next_record().is_none());
        assert_eq!(r.buffered(), 0);
    }

    #[test]
    fn json_reader_handles_chunked_arrival() {
        let mut r = JsonFrameReader::new();
        // Record arrives in three byte slices, none of which are
        // record-aligned.
        r.push(b"{\"hel").unwrap();
        assert!(r.next_record().is_none());
        r.push(b"lo\":\"wor").unwrap();
        assert!(r.next_record().is_none());
        r.push(b"ld\"}\x1e{\"keep\":true}").unwrap();
        assert_eq!(
            r.next_record().as_deref(),
            Some(&b"{\"hello\":\"world\"}"[..])
        );
        // Second record has no trailing separator yet.
        assert!(r.next_record().is_none());
        r.push(b"\x1e").unwrap();
        assert_eq!(r.next_record().as_deref(), Some(&b"{\"keep\":true}"[..]));
    }

    #[test]
    fn json_reader_drops_empty_records() {
        // Two separators back-to-back is a no-op.
        let mut r = JsonFrameReader::new();
        r.push(b"\x1e\x1e").unwrap();
        assert!(r.next_record().is_none());
    }

    #[test]
    fn json_reader_rejects_oversize_record() {
        let mut r = JsonFrameReader::new();
        // Push one byte more than the cap with no separator in
        // sight; the reader must give up rather than buffer
        // indefinitely.
        let blob = vec![b'x'; MAX_RECORD_BYTES + 1];
        let err = r.push(&blob).unwrap_err();
        assert!(matches!(err, FramingError::RecordTooLarge(_)));
    }

    // ---- MessagePack framing ---------------------------------------------

    #[test]
    fn varint_round_trips_short() {
        for v in [0usize, 1, 127, 128, 200, 16_383, 16_384, 1_000_000] {
            let mut out = Vec::new();
            encode_varint(v, &mut out);
            let (decoded, n) = decode_varint(&out).unwrap().unwrap();
            assert_eq!(decoded, v, "value {v}");
            assert_eq!(n, out.len());
        }
    }

    #[test]
    fn msgpack_writer_prepends_length() {
        let payload = vec![0xC0, 0xC1, 0xC2]; // arbitrary 3-byte msgpack
        let framed = write_msgpack_record(&payload).unwrap();
        // 3 < 128 so the prefix is exactly one byte equal to the length.
        assert_eq!(framed[0], 3);
        assert_eq!(&framed[1..], &payload[..]);
    }

    #[test]
    fn msgpack_writer_uses_multi_byte_prefix_for_large_records() {
        let payload = vec![0u8; 200];
        let framed = write_msgpack_record(&payload).unwrap();
        // 200 = 0xC8 → varint = [0xC8, 0x01]
        assert_eq!(framed[0], 0xC8);
        assert_eq!(framed[1], 0x01);
        assert_eq!(framed.len(), payload.len() + 2);
    }

    #[test]
    fn msgpack_reader_splits_chunked_records() {
        let r1 = write_msgpack_record(&[1, 2, 3]).unwrap();
        let r2 = write_msgpack_record(&[4, 5]).unwrap();
        let mut wire = Vec::new();
        wire.extend(&r1);
        wire.extend(&r2);

        let mut reader = MsgPackFrameReader::new();
        // Arrive byte by byte.
        for b in &wire {
            reader.push(std::slice::from_ref(b)).unwrap();
        }
        assert_eq!(reader.next_record().as_deref(), Some(&[1u8, 2, 3][..]));
        assert_eq!(reader.next_record().as_deref(), Some(&[4u8, 5][..]));
        assert!(reader.next_record().is_none());
        assert_eq!(reader.buffered(), 0);
    }

    #[test]
    fn msgpack_reader_rejects_oversize_length() {
        // Hand-craft a varint that decodes to a value larger than
        // MAX_RECORD_BYTES. Encoding `MAX_RECORD_BYTES + 1` is
        // straightforward.
        let mut buf = Vec::new();
        encode_varint(MAX_RECORD_BYTES + 1, &mut buf);
        let mut r = MsgPackFrameReader::new();
        let err = r.push(&buf).unwrap_err();
        assert!(matches!(err, FramingError::RecordTooLarge(_)));
    }

    #[test]
    fn msgpack_reader_rejects_runaway_varint() {
        // Six continuation bytes — already past VARINT_MAX_BYTES.
        let runaway = [0x80, 0x80, 0x80, 0x80, 0x80, 0x80];
        let mut r = MsgPackFrameReader::new();
        let err = r.push(&runaway).unwrap_err();
        assert!(matches!(err, FramingError::VarintTooLong));
    }
}
