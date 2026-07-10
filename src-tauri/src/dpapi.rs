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
    Ok(crate::base64::base64_encode(&encrypted))
}

/// Base64 → 解密字符串
pub fn decrypt_string(encoded: &str) -> Result<String, String> {
    if encoded.is_empty() {
        return Ok(String::new());
    }
    let encrypted = crate::base64::base64_decode(encoded)?;
    let decrypted = decrypt(&encrypted)?;
    String::from_utf8(decrypted).map_err(|e| format!("UTF-8 decode error: {}", e))
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
