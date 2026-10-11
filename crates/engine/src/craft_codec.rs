//! Versioned, bounded binary envelope. Native document bytes stay binary (not JSON arrays).
use serde_json::Value;
pub const LIMIT: usize = 512 * 1024 * 1024;
pub fn pack(meta: &Value, blobs: &[Vec<u8>]) -> Result<Vec<u8>, String> {
    let json = serde_json::to_vec(meta).map_err(|e| e.to_string())?;
    let total =
        blobs.iter().try_fold(16usize.saturating_add(json.len()), |n, b| n.checked_add(4)?.checked_add(b.len())).ok_or("session too large")?;
    if total > LIMIT || blobs.len() > 100_000 {
        return Err("Session exceeds recovery limits; save documents before leaving".into());
    }
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"CRAFTS01");
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(&json);
    out.extend_from_slice(&(blobs.len() as u32).to_le_bytes());
    for b in blobs {
        out.extend_from_slice(&(b.len() as u32).to_le_bytes());
        out.extend_from_slice(b);
    }
    Ok(out)
}
pub fn unpack(bytes: &[u8]) -> Result<(Value, Vec<&[u8]>), String> {
    if bytes.len() > LIMIT || bytes.get(..8) != Some(b"CRAFTS01") {
        return Err("Unsupported session format".into());
    }
    let mut offset = 8usize;
    fn take<'a>(bytes: &'a [u8], offset: &mut usize, size: usize) -> Result<&'a [u8], String> {
        let end = offset.checked_add(size).ok_or("Invalid session length")?;
        let value = bytes.get(*offset..end).ok_or("Incomplete session")?;
        *offset = end;
        Ok(value)
    }
    fn size(bytes: &[u8], offset: &mut usize) -> Result<usize, String> {
        let b: [u8; 4] = take(bytes, offset, 4)?.try_into().map_err(|_| "Invalid length")?;
        Ok(u32::from_le_bytes(b) as usize)
    }
    let n = size(bytes, &mut offset)?;
    let meta = serde_json::from_slice(take(bytes, &mut offset, n)?).map_err(|e| e.to_string())?;
    let count = size(bytes, &mut offset)?;
    if count > 100_000 {
        return Err("Too many session objects".into());
    }
    let mut blobs = Vec::with_capacity(count);
    for _ in 0..count {
        let n = size(bytes, &mut offset)?;
        blobs.push(take(bytes, &mut offset, n)?);
    }
    if offset != bytes.len() {
        return Err("Unexpected session data".into());
    }
    Ok((meta, blobs))
}
pub fn blob<'a>(blobs: &'a [&[u8]], id: usize) -> Result<&'a [u8], String> {
    blobs.get(id).copied().ok_or_else(|| "Missing session object".into())
}
pub fn decode<T: serde::de::DeserializeOwned>(v: &Value) -> Result<T, String> {
    serde_json::from_value(v.clone()).map_err(|e| e.to_string())
}

/// Bound retained buffers while building, before allocating the final envelope.
pub fn push(blobs: &mut Vec<Vec<u8>>, bytes: Vec<u8>) -> Result<usize, String> {
    let used = blobs.iter().try_fold(bytes.len(), |n, b| n.checked_add(b.len())).ok_or("Session too large")?;
    if used > LIMIT || blobs.len() >= 100_000 {
        return Err("Session exceeds recovery limits; history has not been truncated".into());
    }
    let id = blobs.len();
    blobs.push(bytes);
    Ok(id)
}
