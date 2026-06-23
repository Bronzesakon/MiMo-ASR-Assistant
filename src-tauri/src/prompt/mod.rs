use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptTemplate {
    pub system: String,
    pub user_prefix: String,
    pub user_suffix: String,
}

impl Default for PromptTemplate {
    fn default() -> Self {
        Self {
            system: "你是一个专业的文本润色助手。请对用户提供的语音转文字结果进行润色，修正错别字、调整语序、补充标点，使其更加通顺自然。保持原意不变，不要添加额外内容。".to_string(),
            user_prefix: String::new(),
            user_suffix: String::new(),
        }
    }
}

/// 构建完整的提示词
pub fn build_prompt(template: &PromptTemplate, user_input: &str) -> String {
    let mut prompt = String::new();

    if !template.system.is_empty() {
        prompt.push_str(&format!("{}\n\n", template.system));
    }

    if !template.user_prefix.is_empty() {
        prompt.push_str(&format!("{}\n", template.user_prefix));
    }

    prompt.push_str(user_input);

    if !template.user_suffix.is_empty() {
        prompt.push_str(&format!("\n{}", template.user_suffix));
    }

    prompt
}

/// 语音转文字后处理的默认模板
pub fn default_transcription_template() -> PromptTemplate {
    PromptTemplate {
        system: "你是一个专业的文本润色助手。请对用户提供的语音转文字结果进行润色，修正错别字、调整语序、补充标点，使其更加通顺自然。保持原意不变，不要添加额外内容。".to_string(),
        user_prefix: "请润色以下语音转文字结果：".to_string(),
        user_suffix: String::new(),
    }
}

/// 标点恢复的默认模板
pub fn default_punctuation_template() -> PromptTemplate {
    PromptTemplate {
        system: "你是一个文本处理助手。请为用户提供的文本补充适当的标点符号，使其更易阅读。只添加标点，不要修改文本内容。".to_string(),
        user_prefix: "请为以下文本添加标点：".to_string(),
        user_suffix: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_prompt() {
        let template = PromptTemplate {
            system: "System".to_string(),
            user_prefix: "Prefix".to_string(),
            user_suffix: "Suffix".to_string(),
        };
        let result = build_prompt(&template, "Input");
        assert!(result.contains("System"));
        assert!(result.contains("Prefix"));
        assert!(result.contains("Input"));
        assert!(result.contains("Suffix"));
    }

    #[test]
    fn test_build_prompt_empty_system() {
        let template = PromptTemplate {
            system: String::new(),
            user_prefix: "Prefix".to_string(),
            user_suffix: String::new(),
        };
        let result = build_prompt(&template, "Input");
        assert!(!result.contains("System"));
        assert!(result.contains("Prefix"));
        assert!(result.contains("Input"));
    }
}
