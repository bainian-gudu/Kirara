//! Minimal coded-error support for modules ported from the upstream native
//! installer. The Kirara frontend still receives the display text from
//! `TACommandError`; these codes exist so staging and commit failures keep
//! the same machine-readable context during recovery and tests.

use std::fmt;

pub const FILE_IO_FAILED: &str = "FILE_IO_FAILED";
pub const FILE_IN_USE: &str = "FILE_IN_USE";
pub const INSTALL_PATH_INVALID: &str = "INSTALL_PATH_INVALID";
pub const STAGING_IN_USE: &str = "STAGING_IN_USE";
pub const TEMP_DIR_UNAVAILABLE: &str = "TEMP_DIR_UNAVAILABLE";

#[derive(Debug)]
pub struct Coded {
    pub code: &'static str,
    pub detail: Option<String>,
    pub subject: Option<String>,
    source: Option<anyhow::Error>,
}

impl Coded {
    pub fn bare(code: &'static str) -> Self {
        Self {
            code,
            detail: None,
            subject: None,
            source: None,
        }
    }

    pub fn bare_with(code: &'static str, subject: impl Into<String>) -> Self {
        Self {
            code,
            detail: None,
            subject: Some(subject.into()),
            source: None,
        }
    }

    pub fn wrap(mut self, error: anyhow::Error) -> anyhow::Error {
        if self.detail.is_none() {
            self.detail = Some(format!("{error:#}"));
        }
        self.source = Some(error);
        anyhow::Error::new(self)
    }
}

impl fmt::Display for Coded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code)
    }
}

impl std::error::Error for Coded {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source.as_ref().map(|error| error.as_ref())
    }
}

pub trait Attach<Out> {
    fn attach(self, code: &'static str) -> Out;
    fn attach_with(self, code: &'static str, subject: impl Into<String>) -> Out;
}

impl Attach<anyhow::Error> for anyhow::Error {
    fn attach(self, code: &'static str) -> anyhow::Error {
        Coded::bare(code).wrap(self)
    }

    fn attach_with(self, code: &'static str, subject: impl Into<String>) -> anyhow::Error {
        Coded::bare_with(code, subject).wrap(self)
    }
}

impl<T, E> Attach<anyhow::Result<T>> for std::result::Result<T, E>
where
    E: Into<anyhow::Error>,
{
    fn attach(self, code: &'static str) -> anyhow::Result<T> {
        self.map_err(|error| Attach::attach(error.into(), code))
    }

    fn attach_with(self, code: &'static str, subject: impl Into<String>) -> anyhow::Result<T> {
        self.map_err(|error| Attach::attach_with(error.into(), code, subject))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Extracted {
    Coded {
        code: &'static str,
        subject: Option<String>,
    },
    Uncoded {
        detail: String,
    },
}

pub fn extract(error: &anyhow::Error) -> Extracted {
    for cause in error.chain() {
        if let Some(coded) = cause.downcast_ref::<Coded>() {
            return Extracted::Coded {
                code: coded.code,
                subject: coded.subject.clone(),
            };
        }
    }
    Extracted::Uncoded {
        detail: format!("{error:#}"),
    }
}

pub fn code_for_local_io(error: &std::io::Error) -> &'static str {
    match error.raw_os_error() {
        Some(32) | Some(33) => FILE_IN_USE,
        _ => FILE_IO_FAILED,
    }
}
