use super::error::DiagnosticError;
pub(super) struct Cursor<'a> {
    input: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    pub(super) const fn new(input: &'a [u8]) -> Self {
        Self { input, offset: 0 }
    }

    pub(super) fn take(&mut self, length: usize) -> Result<&'a [u8], DiagnosticError> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or(DiagnosticError::Truncated)?;
        let bytes = self
            .input
            .get(self.offset..end)
            .ok_or(DiagnosticError::Truncated)?;
        self.offset = end;
        Ok(bytes)
    }

    pub(super) fn u16(&mut self) -> Result<u16, DiagnosticError> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    pub(super) fn u32(&mut self) -> Result<u32, DiagnosticError> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    pub(super) fn u64(&mut self) -> Result<u64, DiagnosticError> {
        let bytes = self.take(8)?;
        Ok(u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    pub(super) fn remaining(&self) -> usize {
        self.input.len().saturating_sub(self.offset)
    }
}
