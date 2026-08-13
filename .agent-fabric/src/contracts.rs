use crate::error::{FabricError, FabricResult};
use crate::json::Json;
use std::collections::BTreeMap;

fn sensitive_name(name: &str) -> bool {
    let value = name.to_ascii_lowercase();
    [
        "authorization",
        "cookie",
        "credential",
        "password",
        "secret",
        "token",
    ]
    .iter()
    .any(|part| value.contains(part))
}

/// 维护工具只对诊断数据做保守脱敏；日常 Task/Result/Review 不经过 Rust 校验。
pub fn redact(value: &Json) -> Json {
    match value {
        Json::Object(values) => Json::Object(
            values
                .iter()
                .map(|(key, value)| {
                    let value = if sensitive_name(key) {
                        Json::from("[REDACTED]")
                    } else {
                        redact(value)
                    };
                    (key.clone(), value)
                })
                .collect::<BTreeMap<_, _>>(),
        ),
        Json::Array(values) => Json::Array(values.iter().map(redact).collect()),
        _ => value.clone(),
    }
}

/// 路径片段只允许稳定的小写标识，避免诊断留存逃出项目目录。
pub fn safe_identifier(value: &str, label: &str) -> FabricResult<String> {
    if value.is_empty()
        || value.len() > 96
        || !value.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '-' | '_')
        })
    {
        return Err(FabricError::new(
            "invalid_identifier",
            format!("{label} 只能使用小写字母、数字、连字符或下划线。"),
        ));
    }
    Ok(value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redaction_hides_sensitive_keys_without_schema_validation() {
        let mut value = Json::object();
        value.insert("answer", Json::from("自由格式结果")).unwrap();
        value.insert("access_token", Json::from("private")).unwrap();
        let redacted = redact(&value);
        assert_eq!(
            redacted.get("access_token").unwrap().as_str().unwrap(),
            "[REDACTED]"
        );
        assert_eq!(
            redacted.get("answer").unwrap().as_str().unwrap(),
            "自由格式结果"
        );
    }

    #[test]
    fn identifiers_cannot_be_paths() {
        assert!(safe_identifier("../trace", "run id").is_err());
        assert_eq!(safe_identifier("probe-1", "run id").unwrap(), "probe-1");
    }
}
