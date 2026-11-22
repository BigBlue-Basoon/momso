use std::io::Read;

/// Check a stream without splitting a UTF-8 character between reads.
pub fn is_utf8_stream(reader: &mut impl Read) -> std::io::Result<bool> {
    let mut block = vec![0u8; 64 * 1024 + 4];
    let mut pending = 0;
    loop {
        let count = reader.read(&mut block[pending..])?;
        if count == 0 {
            return Ok(pending == 0);
        }
        let end = pending + count;
        match std::str::from_utf8(&block[..end]) {
            Ok(_) => pending = 0,
            Err(error) if error.error_len().is_some() => return Ok(false),
            Err(error) => {
                let valid = error.valid_up_to();
                pending = end - valid;
                block.copy_within(valid..end, 0);
            }
        }
    }
}
