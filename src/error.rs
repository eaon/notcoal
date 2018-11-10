use std::convert::From;
use std::result;

pub type Result<T> = result::Result<T, Error>;

// XXX The following ought to be handled by a macro

#[derive(Debug)]
pub enum Error {
    IoError(std::io::Error),
    JSONError(serde_json::Error),
    RegexError(regex::Error),
    NotmuchError(notmuch::Error),
    UnspecifiedError,
}

impl From<serde_json::Error> for Error {
    fn from(s: serde_json::Error) -> Error {
        Error::JSONError(s)
    }
}

impl From<std::io::Error> for Error {
    fn from(s: std::io::Error) -> Error {
        Error::IoError(s)
    }
}

impl From<regex::Error> for Error {
    fn from(s: regex::Error) -> Error {
        Error::RegexError(s)
    }
}

impl From<notmuch::Error> for Error {
    fn from(s: notmuch::Error) -> Error {
        Error::NotmuchError(s)
    }
}
