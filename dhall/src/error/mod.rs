#[cfg(all(not(target_arch = "wasm32"), feature = "std"))]
use std::io::Error as IOError;

use alloc::format;
use alloc::string::String;

use crate::semantics::resolve::{CyclesStack, ImportLocation};
use crate::syntax::{Import, ParseError};

mod builder;
pub use builder::*;

pub type Result<T> = core::result::Result<T, Error>;

#[derive(Debug)]
pub struct Error {
    kind: ErrorKind,
}

#[derive(Debug)]
#[non_exhaustive]
pub enum ErrorKind {
    #[cfg(all(not(target_arch = "wasm32"), feature = "std"))]
    IO(IOError),
    Parse(ParseError),
    Decode(DecodeError),
    Encode(EncodeError),
    Resolve(ImportError),
    Typecheck(TypeError),
    #[cfg(all(not(target_arch = "wasm32"), feature = "std"))]
    Cache(CacheError),
}

#[derive(Debug)]
pub enum ImportError {
    Missing,
    MissingEnvVar,
    MissingHome,
    SanityCheck,
    UnexpectedImport(Import<()>),
    ImportCycle(CyclesStack, ImportLocation),
    #[cfg(all(not(target_arch = "wasm32"), feature = "std"))]
    Url(url::ParseError),
}

#[derive(Debug)]
pub enum DecodeError {
    CBORError(minicbor::decode::Error),
    WrongFormatError(String),
}

#[derive(Debug)]
pub enum EncodeError {
    CBORError(minicbor::encode::Error<core::convert::Infallible>),
}

/// A structured type error
#[derive(Debug)]
pub struct TypeError {
    message: TypeMessage,
}

/// The specific type error
#[derive(Debug)]
pub enum TypeMessage {
    Custom(String),
}

#[cfg(all(not(target_arch = "wasm32"), feature = "std"))]
#[derive(Debug)]
pub enum CacheError {
    MissingConfiguration,
    InitialisationError { cause: IOError },
    CacheHashInvalid,
}

impl Error {
    pub fn new(kind: ErrorKind) -> Self {
        Error { kind }
    }
    pub fn kind(&self) -> &ErrorKind {
        &self.kind
    }
}

impl TypeError {
    pub fn new(message: TypeMessage) -> Self {
        TypeError { message }
    }
}

impl core::fmt::Display for TypeError {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        use TypeMessage::*;
        let msg = match &self.message {
            Custom(s) => format!("Type error: {}", s),
        };
        write!(f, "{}", msg)
    }
}

#[cfg(feature = "std")]
impl std::error::Error for TypeError {}

impl core::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        let msg = match self {
            EncodeError::CBORError(e) => format!("Encode error: {}", e),
        };
        write!(f, "{}", msg)
    }
}

#[cfg(feature = "std")]
impl std::error::Error for EncodeError {}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        match &self.kind {
            #[cfg(all(not(target_arch = "wasm32"), feature = "std"))]
            ErrorKind::IO(err) => write!(f, "{}", err),
            ErrorKind::Parse(err) => write!(f, "{}", err),
            ErrorKind::Decode(err) => write!(f, "{:?}", err),
            ErrorKind::Encode(err) => write!(f, "{:?}", err),
            ErrorKind::Resolve(err) => write!(f, "{:?}", err),
            ErrorKind::Typecheck(err) => write!(f, "{}", err),
            #[cfg(all(not(target_arch = "wasm32"), feature = "std"))]
            ErrorKind::Cache(err) => write!(f, "{:?}", err),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Error {}
impl From<ErrorKind> for Error {
    fn from(kind: ErrorKind) -> Error {
        Error::new(kind)
    }
}
#[cfg(all(not(target_arch = "wasm32"), feature = "std"))]
impl From<IOError> for Error {
    fn from(err: IOError) -> Error {
        ErrorKind::IO(err).into()
    }
}
impl From<ParseError> for Error {
    fn from(err: ParseError) -> Error {
        ErrorKind::Parse(err).into()
    }
}
#[cfg(all(not(target_arch = "wasm32"), feature = "std"))]
impl From<url::ParseError> for Error {
    fn from(err: url::ParseError) -> Error {
        ErrorKind::Resolve(ImportError::Url(err)).into()
    }
}
impl From<DecodeError> for Error {
    fn from(err: DecodeError) -> Error {
        ErrorKind::Decode(err).into()
    }
}
impl From<EncodeError> for Error {
    fn from(err: EncodeError) -> Error {
        ErrorKind::Encode(err).into()
    }
}
impl From<ImportError> for Error {
    fn from(err: ImportError) -> Error {
        ErrorKind::Resolve(err).into()
    }
}
impl From<TypeError> for Error {
    fn from(err: TypeError) -> Error {
        ErrorKind::Typecheck(err).into()
    }
}
#[cfg(all(not(target_arch = "wasm32"), feature = "std"))]
impl From<CacheError> for Error {
    fn from(err: CacheError) -> Error {
        ErrorKind::Cache(err).into()
    }
}
