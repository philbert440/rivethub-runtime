use config::Environment;
use config::LogLevel;
use tracing_subscriber::filter::LevelFilter;

const DEFAULT_LEVEL: LogLevel = LogLevel::Info;

fn filter(level: LogLevel) -> LevelFilter {
    match level {
        LogLevel::Error => LevelFilter::ERROR,
        LogLevel::Warn => LevelFilter::WARN,
        LogLevel::Info => LevelFilter::INFO,
        LogLevel::Debug => LevelFilter::DEBUG,
    }
}

pub fn init(env: &Environment) {
    let setting = env.log_level();
    tracing_subscriber::fmt()
        .with_max_level(filter(setting.level))
        .with_writer(std::io::stderr)
        .init();
    if let Some(value) = setting.ignored {
        tracing::warn!(
            "unknown RIVETOS_LOG_LEVEL \"{value}\" — logging at {}",
            DEFAULT_LEVEL
        );
    }
}
