use std::result;

pub type Result<T> = result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    IoError(std::io::Error),
    JSONError(serde_json::Error),
    RegexError(regex::Error),
    UnspecifiedError,
}
