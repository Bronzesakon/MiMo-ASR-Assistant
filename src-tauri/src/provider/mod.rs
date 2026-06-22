use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServiceProvider {
    MimoApi,
    MimoTokenPlan,
    DeepSeek,
}

impl std::fmt::Display for ServiceProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServiceProvider::MimoApi => write!(f, "MiMo API"),
            ServiceProvider::MimoTokenPlan => write!(f, "MiMo Token Plan"),
            ServiceProvider::DeepSeek => write!(f, "DeepSeek"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderInfo {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub key_prefix: String,
    pub asr_models: Vec<String>,
    pub chat_models: Vec<String>,
}


/// 根据 API Key 前缀自动识别服务商
pub fn detect_provider(api_key: &str) -> ServiceProvider {
    if api_key.starts_with("tp-") {
        ServiceProvider::MimoTokenPlan
    } else if api_key.starts_with("sk-") {
        // DeepSeek key 也是 sk- 前缀，但无法自动区分
        // 默认归为 MiMo API，用户可手动切换
        ServiceProvider::MimoApi
    } else {
        ServiceProvider::MimoApi
    }
}

/// 获取所有可用服务商信息
pub fn get_providers() -> Vec<ProviderInfo> {
    vec![
        ProviderInfo {
            id: "mimo-api".to_string(),
            name: "MiMo API".to_string(),
            base_url: "https://api.xiaomimimo.com/v1".to_string(),
            key_prefix: "sk-".to_string(),
            asr_models: vec!["mimo-v2.5-asr".to_string()],
            chat_models: vec!["mimo-v2.5".to_string(), "mimo-v2.5-pro".to_string()],
        },
        ProviderInfo {
            id: "mimo-token-plan".to_string(),
            name: "MiMo Token Plan".to_string(),
            base_url: "https://token-plan-cn.xiaomimimo.com/v1".to_string(),
            key_prefix: "tp-".to_string(),
            asr_models: vec!["mimo-v2.5-asr".to_string()],
            chat_models: vec!["mimo-v2.5".to_string(), "mimo-v2.5-pro".to_string()],
        },
        ProviderInfo {
            id: "deepseek".to_string(),
            name: "DeepSeek".to_string(),
            base_url: "https://api.deepseek.com".to_string(),
            key_prefix: "sk-".to_string(),
            asr_models: vec![],
            chat_models: vec!["deepseek-v4-pro".to_string(), "deepseek-v4-flash".to_string()],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_provider() {
        assert!(matches!(detect_provider("sk-xxx"), ServiceProvider::MimoApi));
        assert!(matches!(detect_provider("tp-xxx"), ServiceProvider::MimoTokenPlan));
        assert!(matches!(detect_provider("other"), ServiceProvider::MimoApi));
    }

}
