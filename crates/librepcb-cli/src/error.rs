//! Errors at the command-line boundary (apps/librepcb-cli/commandlineinterface.cpp).
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Project(Box<librepcb_core::project::Error>),
    #[error(transparent)]
    Library(Box<librepcb_core::library::Error>),
    #[error(transparent)]
    FileIo(Box<librepcb_core::fileio::Error>),
    #[error(transparent)]
    Serialization(Box<librepcb_core::serialization::Error>),
    #[error(transparent)]
    Export(Box<librepcb_core::export::Error>),
    #[error(transparent)]
    Job(Box<librepcb_core::job::Error>),
    #[error(transparent)]
    Graphics(Box<librepcb_graphics::Error>),
}
macro_rules! from {
    ($variant:ident,$ty:ty) => {
        impl From<$ty> for Error {
            fn from(value: $ty) -> Self {
                Self::$variant(Box::new(value))
            }
        }
    };
}
from!(Project, librepcb_core::project::Error);
from!(Library, librepcb_core::library::Error);
from!(FileIo, librepcb_core::fileio::Error);
from!(Serialization, librepcb_core::serialization::Error);
from!(Export, librepcb_core::export::Error);
from!(Job, librepcb_core::job::Error);
from!(Graphics, librepcb_graphics::Error);
pub type Result<T> = std::result::Result<T, Error>;
