use std::path::Path;
use std::process::Command;
use ffmpeg_sidecar::command::FfmpegCommand;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[derive(Debug)]
pub enum FfmpegError {
    BinaryNotFound,
    InputNotFound(String),
    OutputDirNotFound(String),
    FfmpegFailed(String),
    InvalidOutput(String),
}

impl std::fmt::Display for FfmpegError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FfmpegError::BinaryNotFound => write!(f, "FFmpeg 未找到，请确保 ffmpeg.exe 在 resources 目录中"),
            FfmpegError::InputNotFound(path) => write!(f, "输入文件不存在: {}", path),
            FfmpegError::OutputDirNotFound(path) => write!(f, "输出目录不存在: {}", path),
            FfmpegError::FfmpegFailed(msg) => write!(f, "FFmpeg 执行失败: {}", msg),
            FfmpegError::InvalidOutput(msg) => write!(f, "无效输出: {}", msg),
        }
    }
}

/// 音频格式信息
#[derive(Debug)]
pub struct AudioInfo {
    pub sample_rate: u32,
    pub channels: u32,
    pub bits_per_sample: u32,
    pub codec: String,
    pub is_wav: bool,
}

/// 获取音频文件的格式信息
pub fn get_audio_info(path: &Path) -> Result<AudioInfo, FfmpegError> {
    if !path.exists() {
        return Err(FfmpegError::InputNotFound(path.display().to_string()));
    }

    let path_str = path.to_str()
        .ok_or_else(|| FfmpegError::InvalidOutput("路径包含无效字符".to_string()))?;

    let mut cmd = Command::new("ffprobe");
    cmd.args(&[
        "-v", "error",
        "-select_streams", "a:0",
        "-show_entries", "stream=sample_rate,channels,bits_per_sample,codec_name",
        "-of", "json",
        path_str,
    ]);

    #[cfg(target_os = "windows")]
    cmd.creation_flags(CREATE_NO_WINDOW);

    let output = cmd.output()
        .map_err(|_| FfmpegError::BinaryNotFound)?;

    if !output.status.success() {
        return Err(FfmpegError::FfmpegFailed("获取音频信息失败".to_string()));
    }

    let json_str = String::from_utf8_lossy(&output.stdout);
    let info: serde_json::Value = serde_json::from_str(&json_str)
        .map_err(|_| FfmpegError::InvalidOutput("无法解析 ffprobe 输出".to_string()))?;

    let stream = info.get("streams")
        .and_then(|s| s.as_array())
        .and_then(|a| a.first())
        .ok_or_else(|| FfmpegError::InvalidOutput("未找到音频流".to_string()))?;

    let sample_rate = stream.get("sample_rate")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);

    let channels = stream.get("channels")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;

    let bits_per_sample = stream.get("bits_per_sample")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;

    let codec = stream.get("codec_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let is_wav = codec == "pcm_s16le" || codec == "pcm_s16be";

    Ok(AudioInfo {
        sample_rate,
        channels,
        bits_per_sample,
        codec,
        is_wav,
    })
}

/// 检查音频是否已经是目标格式（16kHz, 单声道, 16-bit WAV）
pub fn is_target_format(info: &AudioInfo) -> bool {
    info.is_wav
        && info.sample_rate == 16000
        && info.channels == 1
        && info.bits_per_sample == 16
}

/// 将任意音频格式转换为 16kHz 单声道 16-bit WAV
/// 如果已经是目标格式，则跳过转换
pub fn convert_to_wav(input_path: &Path, output_path: &Path) -> Result<(), FfmpegError> {
    if !input_path.exists() {
        return Err(FfmpegError::InputNotFound(input_path.display().to_string()));
    }

    // 检查是否已经是目标格式
    let info = get_audio_info(input_path)?;
    if is_target_format(&info) {
        // 已经是目标格式，直接复制
        if input_path != output_path {
            std::fs::copy(input_path, output_path)
                .map_err(|e| FfmpegError::FfmpegFailed(format!("复制文件失败: {}", e)))?;
        }
        return Ok(());
    }

    // 需要转换
    if let Some(parent) = output_path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent)
                .map_err(|_| FfmpegError::OutputDirNotFound(parent.display().to_string()))?;
        }
    }

    let input_str = input_path.to_str()
        .ok_or_else(|| FfmpegError::InvalidOutput("输入路径包含无效字符".to_string()))?;
    let output_str = output_path.to_str()
        .ok_or_else(|| FfmpegError::InvalidOutput("输出路径包含无效字符".to_string()))?;

    let status = FfmpegCommand::new()
        .input(input_str)
        .args(&["-ar", "16000", "-ac", "1", "-sample_fmt", "s16", "-y"])
        .output(output_str)
        .spawn()
        .map_err(|_| FfmpegError::BinaryNotFound)?
        .wait()
        .map_err(|e| FfmpegError::FfmpegFailed(e.to_string()))?;

    if !status.success() {
        return Err(FfmpegError::FfmpegFailed("音频转换失败，请检查文件格式是否支持".to_string()));
    }

    Ok(())
}

/// 切割 WAV 文件
pub fn slice_wav(
    input_path: &Path,
    output_dir: &Path,
    start_seconds: f64,
    duration_seconds: f64,
) -> Result<std::path::PathBuf, FfmpegError> {
    if !input_path.exists() {
        return Err(FfmpegError::InputNotFound(input_path.display().to_string()));
    }

    if !output_dir.exists() {
        std::fs::create_dir_all(output_dir)
            .map_err(|_| FfmpegError::OutputDirNotFound(output_dir.display().to_string()))?;
    }

    let input_str = input_path.to_str()
        .ok_or_else(|| FfmpegError::InvalidOutput("输入路径包含无效字符".to_string()))?;

    let output_filename = format!("slice_{:.1}_{:.1}.wav", start_seconds, start_seconds + duration_seconds);
    let output_path = output_dir.join(&output_filename);
    let output_str = output_path.to_str()
        .ok_or_else(|| FfmpegError::InvalidOutput("输出路径包含无效字符".to_string()))?;

    let status = FfmpegCommand::new()
        .args(&["-i", input_str])
        .args(&["-ss", &format!("{:.3}", start_seconds)])
        .args(&["-t", &format!("{:.3}", duration_seconds)])
        .args(&["-c", "copy", "-y"])
        .output(output_str)
        .spawn()
        .map_err(|_| FfmpegError::BinaryNotFound)?
        .wait()
        .map_err(|e| FfmpegError::FfmpegFailed(e.to_string()))?;

    if !status.success() {
        return Err(FfmpegError::FfmpegFailed("音频切割失败".to_string()));
    }

    Ok(output_path)
}

/// 获取音频时长（秒）
pub fn get_duration(path: &Path) -> Result<f64, FfmpegError> {
    if !path.exists() {
        return Err(FfmpegError::InputNotFound(path.display().to_string()));
    }

    let path_str = path.to_str()
        .ok_or_else(|| FfmpegError::InvalidOutput("路径包含无效字符".to_string()))?;

    let mut cmd = Command::new("ffprobe");
    cmd.args(&[
        "-v", "error",
        "-show_entries", "format=duration",
        "-of", "default=noprint_wrappers=1:nokey=1",
        path_str,
    ]);

    #[cfg(target_os = "windows")]
    cmd.creation_flags(CREATE_NO_WINDOW);

    let output = cmd.output()
        .map_err(|_| FfmpegError::BinaryNotFound)?;

    if !output.status.success() {
        return Err(FfmpegError::FfmpegFailed("获取音频时长失败".to_string()));
    }

    let duration_str = String::from_utf8_lossy(&output.stdout);
    let duration = duration_str.trim().parse::<f64>()
        .map_err(|_| FfmpegError::InvalidOutput("无法解析音频时长".to_string()))?;

    Ok(duration)
}
