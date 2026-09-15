use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum ConvertError {
    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error(transparent)]
    Dictionary(#[from] manbo_dictionary::DictionaryError),

    #[error(transparent)]
    LanguageModel(#[from] manbo_lm::LmError),

    #[error(transparent)]
    Glossary(#[from] manbo_translate::GlossaryError),

    #[error(transparent)]
    Neural(#[from] manbo_neural::NeuralError),

    /// 文件不是预期格式。
    #[error("{path}:{line}: {reason}")]
    Format {
        /// 出错的文件。
        path: PathBuf,

        /// 从 1 开始的行号。
        line: usize,

        /// 具体原因。
        reason: String,
    },
}
