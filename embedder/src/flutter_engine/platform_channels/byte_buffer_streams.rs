use crate::flutter_engine::platform_channels::byte_streams::{ByteStreamReader, ByteStreamWriter};

pub struct ByteBufferStreamReader<'b> {
    buffer: &'b [u8],
    location: usize,
}

impl<'b> ByteBufferStreamReader<'b> {
    pub fn new(buffer: &'b [u8]) -> Self {
        Self {
            buffer,
            location: 0,
        }
    }
}

impl ByteStreamReader for ByteBufferStreamReader<'_> {
    fn read_byte(&mut self) -> u8 {
        if self.location >= self.buffer.len() {
            eprintln!("Invalid read in StandardCodecByteStreamReader");
            return 0;
        }
        let value = self.buffer[self.location];
        self.location += 1;
        value
    }

    fn read_bytes(&mut self, buffer: &mut [u8], count: usize) {
        if self.location + count > self.buffer.len() {
            eprintln!("Invalid read in StandardCodecByteStreamReader");
            return;
        }
        buffer.copy_from_slice(&self.buffer[self.location..self.location + count]);
        self.location += count;
    }

    fn read_alignment(&mut self, alignment: u8) {
        let mod_ = self.location % alignment as usize;
        if mod_ != 0 {
            self.location += alignment as usize - mod_;
        }
    }
}

pub struct ByteBufferStreamWriter<'b> {
    buffer: &'b mut Vec<u8>,
}

impl<'b> ByteBufferStreamWriter<'b> {
    pub fn new(buffer: &'b mut Vec<u8>) -> Self {
        Self { buffer }
    }
}

impl ByteStreamWriter for ByteBufferStreamWriter<'_> {
    fn write_byte(&mut self, value: u8) {
        self.buffer.push(value);
    }

    fn write_bytes(&mut self, buffer: &[u8], count: usize) {
        self.buffer.extend_from_slice(&buffer[..count]);
    }

    fn write_alignment(&mut self, alignment: u8) {
        let mod_ = self.buffer.len() % alignment as usize;
        if mod_ != 0 {
            for _ in 0..alignment as usize - mod_ {
                self.buffer.push(0);
            }
        }
    }
}
