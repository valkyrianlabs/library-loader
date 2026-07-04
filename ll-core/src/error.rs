pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    TomlDe(#[from] toml::de::Error),
    #[error(transparent)]
    TomlSer(#[from] toml::ser::Error),
    #[error(transparent)]
    Notify(#[from] notify::Error),
    #[error(transparent)]
    Reqwest(#[from] reqwest::Error),
    #[error(transparent)]
    Zip(#[from] zip::result::ZipError),
    #[error(transparent)]
    ParseInt(#[from] std::num::ParseIntError),
    #[error(transparent)]
    Shellexpand(#[from] shellexpand::LookupError<std::env::VarError>),
    #[error("Internal error: {0}")]
    Other(&'static str),
    #[error("No config found")]
    NoConfig,
    #[error("Would overwrite file")]
    WouldOverwrite,
    #[error("Not logged in")]
    NotLoggedIn,
    #[error("Server Error: {0}")]
    ServerError(u16),
    #[error("No files in library")]
    NoFilesInLibrary,
    #[error("File empty")]
    FileEmpty,
    #[error("Zip archive empty")]
    ZipArchiveEmpty,
    #[error("Zip entry too large: {0} ({1} bytes)")]
    ZipEntryTooLarge(String, u64),
    #[error("Zip archive too large after extraction ({0} bytes)")]
    ZipArchiveTooLarge(u64),
    #[error("No EPW file in zip archive")]
    NoEpwInZipArchive,
    #[error("ECAD type not found")]
    EcadNotFound,
}
