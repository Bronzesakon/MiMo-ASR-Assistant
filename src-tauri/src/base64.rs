//! 共享 Base64 编码/解码（audio 和 dpapi 模块共用）。
//!
//! B-10：用 `base64 = "0.22"` crate 的 STANDARD 引擎替换自研逐字节实现，
//! 消除 9MB 分片约 33% 的额外编码耗时；公开函数签名保持不变。
//! 解码仍容忍空白字符（dpapi 持久化串可能带换行），空串返回空向量。

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;

/// 标准 Base64 编码（带填充）
pub fn base64_encode(data: &[u8]) -> String {
    STANDARD.encode(data)
}

/// 编码后长度（含填充），与 STANDARD 引擎输出一致
pub fn base64_encoded_len(input_len: usize) -> usize {
    input_len.div_ceil(3) * 4
}

/// 标准 Base64 解码：先过滤空白字符（兼容旧实现的宽松行为），
/// 非法字符/错误填充由 crate 严格拒绝。
pub fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
    let cleaned: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    if cleaned.is_empty() {
        return Ok(vec![]);
    }
    STANDARD
        .decode(cleaned.as_bytes())
        .map_err(|e| format!("Invalid base64: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    // —— 原寄居在 audio/mod.rs 的测试（B-10 迁移至此）——

    #[test]
    fn test_base64_encode() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
    }

    #[test]
    fn test_base64_encoded_len() {
        assert_eq!(base64_encoded_len(0), 0);
        assert_eq!(base64_encoded_len(1), 4);
        assert_eq!(base64_encoded_len(2), 4);
        assert_eq!(base64_encoded_len(3), 4);
        assert_eq!(base64_encoded_len(4), 8);
    }

    // —— 新增：替换 crate 后的行为回归 ——

    #[test]
    fn roundtrip_binary_data() {
        let data: Vec<u8> = (0..=255u8).cycle().take(10_000).collect();
        let encoded = base64_encode(&data);
        assert_eq!(encoded.len(), base64_encoded_len(data.len()));
        assert_eq!(base64_decode(&encoded).expect("合法编码应可解码"), data);
    }

    #[test]
    fn decode_tolerates_whitespace() {
        // dpapi 持久化兼容：空白字符被过滤后正常解码
        assert_eq!(
            base64_decode("Zm9v\n YmFy ").expect("空白应被容忍"),
            b"foobar"
        );
    }

    #[test]
    fn decode_empty_returns_empty() {
        assert_eq!(base64_decode("").unwrap(), Vec::<u8>::new());
        assert_eq!(base64_decode("  \n").unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn decode_rejects_invalid_input() {
        assert!(base64_decode("!!!").is_err());
        assert!(base64_decode("Zg=").is_err()); // 长度非 4 倍数
    }
}
