//! Reads a stream to the end with a cap on what is kept in memory. Past the
//! cap the rest is still read and thrown away, so the writer never sees a
//! broken pipe, and the caller is told the input was too big.

use std::io::Read;

// bytes read per call
const CHUNK_BYTES: usize = 64 * 1024;

/*
 * reads everything from reader, keeping at most cap bytes
 *
 * reader: the stream, e.g. stdin
 * cap: most bytes kept; a longer input is drained to the end, then refused
 * returns: the bytes, or None when the input was longer than cap or a read
 *          failed
 */
pub fn read_capped<R: Read>(reader: &mut R, cap: usize) -> Option<Vec<u8>> {
    let mut kept: Vec<u8> = Vec::new();
    let mut too_big = false;
    let mut chunk = vec![0u8; CHUNK_BYTES];

    loop {
        let read = reader.read(&mut chunk);
        let len = match read {
            Ok(n) => n,
            Err(e) => {
                // a signal arrived mid-read: try again
                if e.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return None;
            }
        };

        // 0 bytes means the end of the input
        if len == 0 {
            break;
        }

        // past the cap, keep reading but stop keeping
        if too_big {
            continue;
        }
        if kept.len() + len > cap {
            too_big = true;
            kept = Vec::new();
            continue;
        }
        kept.extend_from_slice(&chunk[..len]);
    }

    if too_big {
        return None;
    }
    Some(kept)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Error, ErrorKind};

    #[test]
    fn small_input_is_read_whole() {
        let mut input = Cursor::new(b"{\"a\":1}".to_vec());
        assert_eq!(read_capped(&mut input, 64), Some(b"{\"a\":1}".to_vec()));
    }

    #[test]
    fn empty_input_is_empty() {
        let mut input = Cursor::new(Vec::new());
        assert_eq!(read_capped(&mut input, 64), Some(Vec::new()));
    }

    #[test]
    fn input_of_exactly_the_cap_is_kept() {
        let mut input = Cursor::new(vec![7u8; 100]);
        assert_eq!(read_capped(&mut input, 100), Some(vec![7u8; 100]));
    }

    #[test]
    fn input_one_byte_over_the_cap_is_refused() {
        let mut input = Cursor::new(vec![7u8; 101]);
        assert_eq!(read_capped(&mut input, 100), None);
    }

    #[test]
    fn input_over_the_cap_is_still_drained() {
        // a large input, far past the cap, is read to the end
        let mut input = Cursor::new(vec![1u8; 300_000]);

        let read = read_capped(&mut input, 1000);

        assert_eq!(read, None);
        assert_eq!(input.position(), 300_000);
    }

    // a reader that always fails
    struct Broken;
    impl Read for Broken {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            Err(Error::other("broken"))
        }
    }

    #[test]
    fn read_error_gives_none() {
        assert_eq!(read_capped(&mut Broken, 64), None);
    }

    // a reader that is interrupted once, then gives its bytes
    struct InterruptedOnce {
        interrupted: bool,
        inner: Cursor<Vec<u8>>,
    }
    impl Read for InterruptedOnce {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if !self.interrupted {
                self.interrupted = true;
                return Err(Error::new(ErrorKind::Interrupted, "signal"));
            }
            self.inner.read(buf)
        }
    }

    #[test]
    fn interrupted_read_is_retried() {
        let mut input = InterruptedOnce {
            interrupted: false,
            inner: Cursor::new(b"ok".to_vec()),
        };
        assert_eq!(read_capped(&mut input, 64), Some(b"ok".to_vec()));
    }
}
