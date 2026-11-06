//! Complete structured stdout is independent of model-facing previews/spills.
use std::sync::{Arc, Mutex};

/// One runtime-requested, invocation-local byte channel. Only a complete EOF
/// within the caller's bound can be consumed; overflow never yields a preview.
#[derive(Clone)]
pub(crate) struct BoundedStdoutCapture(Arc<Mutex<State>>);

struct State {
    bytes: Vec<u8>,
    limit: usize,
    overflow: bool,
    complete: bool,
}

impl BoundedStdoutCapture {
    pub(crate) fn new(limit: usize) -> Self {
        Self(Arc::new(Mutex::new(State {
            bytes: Vec::new(),
            limit,
            overflow: false,
            complete: false,
        })))
    }

    pub(crate) fn push(&self, bytes: &[u8]) {
        let mut state = self.0.lock().expect("structured stdout capture");
        if state.overflow {
            return;
        }
        if bytes.len() > state.limit - state.bytes.len() {
            state.overflow = true;
            state.bytes.clear();
        } else {
            state.bytes.extend_from_slice(bytes);
        }
    }

    pub(crate) fn finish(&self) {
        self.0.lock().expect("structured stdout capture").complete = true;
    }

    pub(crate) fn take_complete(&self) -> Result<Vec<u8>, &'static str> {
        let mut state = self.0.lock().expect("structured stdout capture");
        if !state.complete {
            return Err("structured stdout capture did not reach EOF");
        }
        if state.overflow {
            return Err("structured stdout exceeds its byte bound");
        }
        Ok(std::mem::take(&mut state.bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::BoundedStdoutCapture;

    #[test]
    fn structured_bytes_require_eof_and_never_substitute_a_truncated_preview() {
        let capture = BoundedStdoutCapture::new(4);
        capture.push(&[0xff, 0x00]);
        capture.push(&[0xc3, 0xa9]);
        assert!(capture.take_complete().is_err());
        capture.finish();
        assert_eq!(capture.take_complete().unwrap(), [0xff, 0x00, 0xc3, 0xa9]);

        let overflow = BoundedStdoutCapture::new(4);
        overflow.push(b"1234");
        overflow.push(b"5");
        overflow.push(b"6789");
        overflow.finish();
        assert_eq!(
            overflow.take_complete(),
            Err("structured stdout exceeds its byte bound")
        );
    }
}
