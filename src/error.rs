//! The crate's error type: a message, its kind and, optionally, the error that
//! caused it.
use std::{error::Error as StdError, fmt};

/// Error returned by every fallible operation.
///
/// `{}` prints the outermost message, such as the stage that failed; `{:#}`
/// prints the whole chain of causes separated by `": "`. The causes are also
/// available through [`std::error::Error::source`], and [`Error::kind`] tells
/// why the operation failed.
#[derive(Debug)]
pub struct Error {
    message: String,
    kind: ErrorKind,
    source: Option<Box<Error>>,
}

/// Why an operation failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorKind {
    /// An input is malformed, unsupported or inconsistent: a source, PILout,
    /// certificate, witness or setting that does not meet the specification,
    /// or an artifact that does not encode the source's relation.
    Invalid,
    /// An input exceeds a size limit of this implementation, such as the
    /// largest file, trace or number of expressions. The limits bound the
    /// memory a check can use; a circuit too large for the reference layout
    /// may still fit a packed layout.
    Limit,
}

/// A fallible compiler, checker or witness operation.
pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: ErrorKind::Invalid,
            source: None,
        }
    }

    /// Why the operation failed: the kind of the innermost cause.
    pub fn kind(&self) -> ErrorKind {
        let mut error = self;
        while let Some(cause) = error.source.as_deref() {
            error = cause;
        }
        error.kind
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
/// are rewritten where they occur, so that they stay out of the public API.
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

/// Return an error with message `why` unless `ok` holds. `why` is formatted
/// only on failure, so callers in loops pass `format_args!` rather than an
/// allocated `String`.
pub(crate) fn ensure(ok: bool, why: impl fmt::Display) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(Error::new(why.to_string()))
    }
}

/// Return a [`ErrorKind::Limit`] error with message `why` unless `ok` holds.
pub(crate) fn ensure_within_limit(ok: bool, why: impl fmt::Display) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(Error {
            kind: ErrorKind::Limit,
            ..Error::new(why.to_string())
        })
    }
}

/// Adds a context message to the error of a `Result`.
pub(crate) trait Context<T> {
    fn context(self, message: impl Into<String>) -> Result<T>;
}
impl<T, E: Into<Error>> Context<T> for std::result::Result<T, E> {
    fn context(self, message: impl Into<String>) -> Result<T> {
        self.map_err(|source| Error {
            source: Some(Box::new(source.into())),
            ..Error::new(message)
        })
    }
}
