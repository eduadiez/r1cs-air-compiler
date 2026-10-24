//! The crate's error type: a message and, optionally, the error that caused it.
use std::{error::Error as StdError, fmt};

/// Error returned by every fallible operation.
///
/// `{}` prints the outermost message, such as the stage that failed; `{:#}`
/// prints the whole chain of causes separated by `": "`. The causes are also
/// available through [`std::error::Error::source`].
#[derive(Debug)]
pub struct Error {
    message: String,
    source: Option<Box<Error>>,
}

/// A fallible compiler, checker or witness operation.
pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            source: None,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)?;
        if f.alternate() {
            let mut cause = self.source.as_deref();
            while let Some(error) = cause {
                write!(f, ": {}", error.message)?;
                cause = error.source.as_deref();
            }
        }
        Ok(())
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.source
            .as_deref()
            .map(|e| e as &(dyn StdError + 'static))
    }
}

impl From<String> for Error {
    fn from(message: String) -> Self {
        Self::new(message)
    }
}
impl From<&str> for Error {
    fn from(message: &str) -> Self {
        Self::new(message)
    }
}

/// Standard library errors keep their message only. Errors from other crates
/// are converted where they occur, so that they stay out of the public API.
macro_rules! from_display {
    ($($t:ty),* $(,)?) => {
        $(impl From<$t> for Error {
            fn from(error: $t) -> Self {
                Self::new(error.to_string())
            }
        })*
    };
}
from_display!(
    std::io::Error,
    std::array::TryFromSliceError,
    std::num::TryFromIntError,
);

/// Return an error with message `why` unless `ok` holds.
pub(crate) fn ensure(ok: bool, why: impl Into<String>) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(Error::new(why))
    }
}

/// Adds a context message to the error of a `Result`.
pub(crate) trait Context<T> {
    fn context(self, message: impl Into<String>) -> Result<T>;
}
impl<T, E: Into<Error>> Context<T> for std::result::Result<T, E> {
    fn context(self, message: impl Into<String>) -> Result<T> {
        self.map_err(|source| Error {
            message: message.into(),
            source: Some(Box::new(source.into())),
        })
    }
}
