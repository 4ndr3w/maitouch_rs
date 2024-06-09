use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
pub struct TouchBuffer {
    data: AtomicU64,
}
impl TouchBuffer {
    pub fn store(&self, buf: &[u8]) {
        if buf.len() != 9 {
            return;
        }
        let mut combined = 0u64;
        for val in &buf[1..8] {
            combined |= u64::from(*val);
            combined <<= 8;
        }
        self.data.store(combined, Ordering::Relaxed);
    }

    pub fn load(&self, buf: &mut Vec<u8>) {
        buf.clear();
        buf.push(b'(');
        let val = self.data.load(Ordering::Relaxed);
        buf.extend_from_slice(&val.to_be_bytes()[..7]);
        buf.push(b')');
    }
}

#[test]
fn test_buf() {
    let buf = TouchBuffer::default();

    let input = vec![b'(', 65, 66, 67, 68, 69, 70, 71, b')'];
    buf.store(&input);

    let mut output = Vec::new();
    buf.load(&mut output);
    assert_eq!(input, output);
}
