use std::path::Path;
use crate::ffmpeg;
use crate::{info, error, warn};

const MAX_CHUNK_SECONDS: u32 = 150;
const MAX_BASE64_BYTES: usize = 9 * 1024 * 1024;
const CHUNK_SHRINK_STEP: u32 = 10;
const MIN_CHUNK_SECONDS: u32 = 5;

#[derive(Debug)]
pub enum AudioError {
    FfmpegError(ffmpeg::FfmpegError),
    IoError(std::io::Error),
    EncodingError(String),
}

impl std::fmt::Display for AudioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AudioError::FfmpegError(e) => write!(f, "{}", e),
            AudioError::IoError(e) => write!(f, "IO 错误: {}", e),
            AudioError::EncodingError(msg) => write!(f, "编码错误: {}", msg),
        }
    }
}

impl From<ffmpeg::FfmpegError> for AudioError {
    fn from(e: ffmpeg::FfmpegError) -> Self {
        AudioError::FfmpegError(e)
    }
}

impl From<std::io::Error> for AudioError {
    fn from(e: std::io::Error) -> Self {
        AudioError::IoError(e)
    }
}

/// 生成唯一临时目录（避免并发冲突）
fn unique_temp_dir() -> std::path::PathBuf {
    let id = format!("{}_{}", std::process::id(), std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos());
    std::env::temp_dir().join(format!("mimo_{}", id))
}

/// 将音频文件转换为 WAV 并切分，通过回调逐片返回 Base64 data URL
/// callback 参数：(当前分片序号, 总分片估算, data_url)
pub fn convert_and_slice(
    input_path: &Path,
    _slice_seconds: Option<u32>,
    callback: &dyn Fn(u32, u32, String),
) -> Result<usize, AudioError> {
    info!("convert_and_slice 开始: {:?}", input_path);

    let temp_dir = unique_temp_dir();
    std::fs::create_dir_all(&temp_dir)?;
    info!("临时目录: {:?}", temp_dir);

    // 检查是否已经是目标格式
    let info_result = ffmpeg::get_audio_info(input_path);
    let needs_conversion = match &info_result {
        Ok(audio_info) => {
            let is_target = audio_info.is_wav
                && audio_info.sample_rate == 16000
                && audio_info.channels == 1
                && audio_info.bits_per_sample == 16;
            if is_target {
                info!("音频已是目标格式 (16kHz/mono/16bit WAV)，跳过转码");
            } else {
                info!("音频格式: codec={}, {}Hz, {}ch, {}bit，需要转码",
                    audio_info.codec, audio_info.sample_rate, audio_info.channels, audio_info.bits_per_sample);
            }
            !is_target
        }
        Err(e) => {
            warn!("无法获取音频信息: {}，尝试转码", e);
            true
        }
    };

    let wav_path = if needs_conversion {
        info!("开始转码...");
        let out = temp_dir.join("converted.wav");
        ffmpeg::convert_to_wav(input_path, &out)?;
        info!("转码完成");
        out
    } else {
        // 直接使用原文件，复制到临时目录以避免路径问题
        let out = temp_dir.join("converted.wav");
        std::fs::copy(input_path, &out)?;
        info!("已复制原文件到临时目录");
        out
    };

    // 获取时长
    let total_duration = ffmpeg::get_duration(&wav_path)?;
    info!("音频时长: {:.1}s", total_duration);

    if total_duration <= 0.0 {
        error!("音频时长为 0");
        cleanup_temp(&temp_dir);
        return Err(AudioError::EncodingError("音频时长为 0".to_string()));
    }

    // 动态切割
    info!("开始分片...");
    // 估算总分片数（用于前端进度条）
    let estimated_total = ((total_duration / MAX_CHUNK_SECONDS as f64).ceil() as u32).max(1);
    let mut current_time = 0.0;
    let mut chunk_sec = MAX_CHUNK_SECONDS;
    let mut chunk_index = 0;

    while current_time < total_duration {
        let remaining = total_duration - current_time;
        if remaining < 1.0 {
            break;
        }
        let mut this_sec = (chunk_sec as f64).min(remaining);

        info!("分片 {}: {:.0}s-{:.0}s ({:.1}s)", chunk_index + 1, current_time, current_time + this_sec, this_sec);

        let mut slice_path;
        let mut audio_bytes;
        let mut b64_len;

        loop {
            slice_path = ffmpeg::slice_wav(&wav_path, &temp_dir, current_time, this_sec)?;
            audio_bytes = std::fs::read(&slice_path)?;
            b64_len = base64_encoded_len(audio_bytes.len());

            if b64_len <= MAX_BASE64_BYTES {
                break;
            }

            this_sec -= CHUNK_SHRINK_STEP as f64;
            if this_sec < MIN_CHUNK_SECONDS as f64 {
                error!("分片 {} 缩至 {:.1}s 仍超限 ({:.1}MB)", chunk_index, this_sec, b64_len as f64 / 1024.0 / 1024.0);
                cleanup_temp(&temp_dir);
                return Err(AudioError::EncodingError(
                    format!("分片 {} 缩至 {:.1}s 仍超限 ({:.1}MB)", chunk_index, this_sec, b64_len as f64 / 1024.0 / 1024.0)
                ));
            }
            warn!("分片 {} Base64={:.1}MB 超限，缩短至 {:.1}s", chunk_index, b64_len as f64 / 1024.0 / 1024.0, this_sec);
        }

        let base64_str = base64_encode(&audio_bytes);
        let data_url = format!("data:audio/wav;base64,{}", base64_str);
        // 立即通过回调交出 data_url，本函数不再持有引用
        callback(chunk_index + 1, estimated_total, data_url);
        // 释放原始音频字节
        drop(audio_bytes);

        info!("分片 {} 完成: {:.0}KB", chunk_index + 1, b64_len as f64 / 1024.0);

        current_time += this_sec;
        chunk_index += 1;
        chunk_sec = MAX_CHUNK_SECONDS;
    }

    info!("convert_and_slice 完成: {} 个分片", chunk_index);

    cleanup_temp(&temp_dir);

    Ok(chunk_index as usize)
}

fn cleanup_temp(path: &std::path::Path) {
    if path.exists() {
        let _ = std::fs::remove_dir_all(path);
    }
}

fn base64_encoded_len(input_len: usize) -> usize {
    (input_len + 2) / 3 * 4
}

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

        if chunk.len() > 1 {
            result.push(CHARS[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }

        if chunk.len() > 2 {
            result.push(CHARS[(triple & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
