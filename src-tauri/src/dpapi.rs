use std::ffi::c_void;

// Windows DPAPI structures
#[repr(C)]
struct DataBlob {
    cb_data: u32,
    pb_data: *mut u8,
}

#[link(name = "crypt32")]
extern "system" {
    fn CryptProtectData(
        data_in: *const DataBlob,
        description: *const u16,
        optional_entropy: *const DataBlob,
        reserved: *const c_void,
        prompt_struct: *const c_void,
        flags: u32,
        data_out: *mut DataBlob,
    ) -> i32;

    fn CryptUnprotectData(
        data_in: *const DataBlob,
        description: *mut *mut u16,
        optional_entropy: *const DataBlob,
        reserved: *const c_void,
        prompt_struct: *const c_void,
        flags: u32,
        data_out: *mut DataBlob,
    ) -> i32;

    fn LocalFree(hmem: *mut c_void) -> *mut c_void;
}

/// 用 Windows DPAPI 加密字节数据
pub fn encrypt(data: &[u8]) -> Result<Vec<u8>, String> {
    let data_in = DataBlob {
        cb_data: data.len() as u32,
        pb_data: data.as_ptr() as *mut u8,
    };
    let mut data_out = DataBlob {
        cb_data: 0,
        pb_data: std::ptr::null_mut(),
    };

    let success = unsafe { CryptProtectData(&data_in, std::ptr::null(), std::ptr::null(), std::ptr::null(), std::ptr::null(), 0, &mut data_out) };

    if success == 0 {
        return Err("CryptProtectData failed".to_string());
    }

    let encrypted = unsafe { std::slice::from_raw_parts(data_out.pb_data, data_out.cb_data as usize) }.to_vec();
    unsafe { LocalFree(data_out.pb_data as *mut c_void) };
    Ok(encrypted)
}

/// 用 Windows DPAPI 解密字节数据
pub fn decrypt(data: &[u8]) -> Result<Vec<u8>, String> {
    let data_in = DataBlob {
        cb_data: data.len() as u32,
        pb_data: data.as_ptr() as *mut u8,
    };
    let mut data_out = DataBlob {
        cb_data: 0,
        pb_data: std::ptr::null_mut(),
    };

    let success = unsafe { CryptUnprotectData(&data_in, std::ptr::null_mut(), std::ptr::null(), std::ptr::null(), std::ptr::null(), 0, &mut data_out) };

    if success == 0 {
        return Err("CryptUnprotectData failed".to_string());
    }

    let decrypted = unsafe { std::slice::from_raw_parts(data_out.pb_data, data_out.cb_data as usize) }.to_vec();
    unsafe { LocalFree(data_out.pb_data as *mut c_void) };
    Ok(decrypted)
}

/// 加密字符串 → Base64
pub fn encrypt_string(plaintext: &str) -> Result<String, String> {
    if plaintext.is_empty() {
        return Ok(String::new());
    }
    let encrypted = encrypt(plaintext.as_bytes())?;
    Ok(base64_encode(&encrypted))
}

/// Base64 → 解密字符串
pub fn decrypt_string(encoded: &str) -> Result<String, String> {
    if encoded.is_empty() {
        return Ok(String::new());
    }
    let encrypted = base64_decode(encoded)?;
    let decrypted = decrypt(&encrypted)?;
    String::from_utf8(decrypted).map_err(|e| format!("UTF-8 decode error: {}", e))
}

// ---- 自带 Base64 编解码，不引入额外 crate ----

fn base64_encode(data: &[u8]) -> String {
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

fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_roundtrip() {
        let original = "sk-test-api-key-12345";
        let encrypted = encrypt_string(original).unwrap();
        assert_ne!(encrypted, original);
        let decrypted = decrypt_string(&encrypted).unwrap();
        assert_eq!(decrypted, original);
    }

    #[test]
    fn test_empty_string() {
        assert_eq!(encrypt_string("").unwrap(), "");
        assert_eq!(decrypt_string("").unwrap(), "");
    }
}
