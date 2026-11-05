//! A sequential big-endian reader with BinData's failure rule: a field read
//! with no bytes left is an `EOFError`; one with only some of its bytes left is
//! an `IOError` ("data truncated"). Measured against the Ruby reference on every
//! truncation length of every 1c table (tests/read/truncation.json).
use super::error::Error;

pub(crate) struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    pub(crate) fn new(data: &'a [u8], pos: usize) -> Self {
        Self { data, pos }
    }
    pub(crate) fn rest(&self) -> &'a [u8] {
        &self.data[self.pos.min(self.data.len())..]
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let remaining = self.data.len().saturating_sub(self.pos);
        if n == 0 {
            return Ok(&[]);
        }
        if remaining == 0 {
            return Err(Error::eof());
        }
        if remaining < n {
            return Err(Error::truncated());
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    pub(crate) fn bytes(&mut self, n: usize) -> Result<&'a [u8], Error> {
        self.take(n)
    }
    /// BinData `skip`: any shortage is an `EOFError`.
    pub(crate) fn skip(&mut self, n: usize) -> Result<(), Error> {
        if self.data.len().saturating_sub(self.pos) < n {
            return Err(Error::eof());
        }
        self.pos += n;
        Ok(())
    }
    pub(crate) fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }
    pub(crate) fn u16(&mut self) -> Result<u16, Error> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }
    pub(crate) fn i16(&mut self) -> Result<i16, Error> {
        Ok(i16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }
    pub(crate) fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub(crate) fn i32(&mut self) -> Result<i32, Error> {
        Ok(i32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub(crate) fn i64(&mut self) -> Result<i64, Error> {
        Ok(i64::from_be_bytes(self.take(8)?.try_into().unwrap()))
    }
}
