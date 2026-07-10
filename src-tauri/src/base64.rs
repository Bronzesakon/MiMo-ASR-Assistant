/// 共享 Base64 编码/解码（audio 和 dpapi 模块共用）
pub fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        result.push(CHARS[((triple >> 18) & 0x3F) as usize] as char);
        result.push(CHARS[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 { result.push(CHARS[((triple >> 6) & 0x3F) as usize] as char); } else { result.push('='); }
        if chunk.len() > 2 { result.push(CHARS[(triple & 0x3F) as usize] as char); } else { result.push('='); }
    }
    result
}

pub fn base64_encoded_len(input_len: usize) -> usize {
    (input_len + 2) / 3 * 4
}

pub fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
    let input: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    if input.is_empty() { return Ok(vec![]); }
    if input.len() % 4 != 0 { return Err("Invalid base64 length".to_string()); }

    fn char_val(c: char) -> Result<u8, String> {
        match c {
            'A'..='Z' => Ok((c as u8) - b'A'),
            'a'..='z' => Ok((c as u8) - b'a' + 26),
            '0'..='9' => Ok((c as u8) - b'0' + 52),
            '+' => Ok(62),
            '/' => Ok(63),
            _ => Err(format!("Invalid base64 char: {}", c)),
        }
    }

    let mut result = Vec::with_capacity(input.len() * 3 / 4);
    for chunk in input.as_bytes().chunks(4) {
        let a = char_val(chunk[0] as char)?;
        let b = char_val(chunk[1] as char)?;
        let c = if chunk[2] == b'=' { 0 } else { char_val(chunk[2] as char)? };
        let d = if chunk[3] == b'=' { 0 } else { char_val(chunk[3] as char)? };
        result.push((a << 2) | (b >> 4));
        if chunk[2] != b'=' { result.push((b << 4) | (c >> 2)); }
        if chunk[3] != b'=' { result.push((c << 6) | d); }
    }
    Ok(result)
}
