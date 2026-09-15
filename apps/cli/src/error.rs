use manbo_dictionary::DictionaryError;
use manbo_learning::LearningError;
use manbo_lm::LmError;
use manbo_neural::NeuralError;
use manbo_platform::ConfigError;
use manbo_predict::PredictError;
use manbo_translate::GlossaryError;
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error(transparent)]
    Dictionary(#[from] DictionaryError),

    #[error(transparent)]
    Neural(#[from] NeuralError),

    #[error(transparent)]
    Glossary(#[from] GlossaryError),

    #[error(transparent)]
    Learning(#[from] LearningError),

    /// 学习语言不是 en / ja。
    #[error("learning language must be en or ja, got {0:?}")]
    Language(String),

    #[error(transparent)]
    Config(#[from] ConfigError),

    #[error(transparent)]
    Predict(#[from] PredictError),

    #[error(transparent)]
    LanguageModel(#[from] LmError),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Replay(#[from] crate::replay::ReplayError),

    #[error(transparent)]
    Eval(#[from] crate::eval::EvalError),

    #[error(transparent)]
    Tune(#[from] crate::tuning::TuneError),

    #[error(transparent)]
    Sync(#[from] manbo_sync::SyncError),
}
