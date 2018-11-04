
#[derive(Debug)]
pub enum Error {
    IoError(std::io::Error),
    JSONError(serde_json::Error),
    UnspecifiedError
}
