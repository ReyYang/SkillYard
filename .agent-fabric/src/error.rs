use std::fmt::{Display, Formatter};

/// Core 的稳定错误语义；CLI 会把它转换为机器可读 JSON。
#[derive(Debug, Clone)]
pub struct FabricError {
    pub code: String,
    pub message: String,
    pub details: Option<String>,
    pub exit_code: i32,
}

impl FabricError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            details: None,
            exit_code: 2,
        }
    }

    pub fn with_details(mut self, details: impl Into<String>) -> Self {
        self.details = Some(details.into());
        self
    }
}

impl Display for FabricError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for FabricError {}

impl From<std::io::Error> for FabricError {
    fn from(error: std::io::Error) -> Self {
        Self::new("io_error", error.to_string())
    }
}

pub type FabricResult<T> = Result<T, FabricError>;
