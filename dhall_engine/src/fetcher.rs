//! Default filesystem/HTTP/env import fetcher.
//!
//! This implements the standard Dhall import resolution behavior:
//! local files, environment variables, HTTP(S) remote imports.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use dhall::error::{Error, ImportError};
use dhall::semantics::{
    Canonicalize, Import, ImportFetcher, ImportLocation, ImportLocationKind,
};
use dhall::syntax::{FilePath, FilePrefix, ImportMode, ImportTarget};

#[cfg(feature = "std")]
use std::path::PathBuf;
#[cfg(not(target_arch = "wasm32"))]
use url::Url;

/// The standard Dhall import fetcher: resolves local files, env vars, and HTTP imports.
pub struct DefaultFetcher;

impl ImportFetcher for DefaultFetcher {
    fn chain(
        &self,
        base: &ImportLocation,
        import: &Import,
    ) -> Result<ImportLocation, Error> {
        // Makes no sense to chain an import if the current file is not dhall code.
        assert!(matches!(base.mode(), ImportMode::Code));
        if matches!(base.kind(), ImportLocationKind::NoImport) {
            return Err(ImportError::UnexpectedImport(import.clone()).into());
        }

        let kind = match &import.location {
            ImportTarget::Local(prefix, path) => {
                chain_local(base.kind(), *prefix, path)?
            }
            #[cfg(not(target_arch = "wasm32"))]
            ImportTarget::Remote(remote) => {
                if matches!(base.kind(), ImportLocationKind::Remote(..))
                    && !matches!(import.mode, ImportMode::Location)
                {
                    return Err(ImportError::SanityCheck.into());
                }
                let mut url = Url::parse(&format!(
                    "{}://{}",
                    remote.scheme, remote.authority
                ))?;
                use itertools::Itertools;
                url.set_path(&remote.path.file_path.iter().join("/"));
                url.set_query(remote.query.as_ref().map(String::as_ref));
                ImportLocationKind::Remote(url)
            }
            ImportTarget::Env(var_name) => {
                #[cfg(not(target_arch = "wasm32"))]
                if matches!(base.kind(), ImportLocationKind::Remote(..))
                    && !matches!(import.mode, ImportMode::Location)
                {
                    return Err(ImportError::SanityCheck.into());
                }
                ImportLocationKind::Env(var_name.clone())
            }
            ImportTarget::Missing => ImportLocationKind::Missing,
        };
        Ok(ImportLocation::new(kind, import.mode))
    }

    fn fetch(&self, location: &ImportLocation) -> Result<String, Error> {
        fetch_text(location.kind())
    }
}

#[cfg(feature = "std")]
fn chain_local(
    base: &ImportLocationKind,
    prefix: FilePrefix,
    path: &FilePath,
) -> Result<ImportLocationKind, Error> {
    Ok(match base {
        ImportLocationKind::Local(..)
        | ImportLocationKind::Env(..)
        | ImportLocationKind::Missing => {
            let dir = match base {
                ImportLocationKind::Local(path) => {
                    path.parent().unwrap().to_owned()
                }
                ImportLocationKind::Env(..)
                | ImportLocationKind::Missing => std::env::current_dir()?,
                _ => unreachable!(),
            };
            let mut dir: Vec<String> = dir
                .components()
                .map(|component| {
                    component.as_os_str().to_string_lossy().into_owned()
                })
                .collect();
            let root = match prefix {
                FilePrefix::Here => dir,
                FilePrefix::Parent => {
                    dir.push("..".to_string());
                    dir
                }
                FilePrefix::Absolute => vec![],
                FilePrefix::Home => vec![],
            };
            let path: Vec<_> = root
                .into_iter()
                .chain(path.file_path.iter().cloned())
                .collect();
            let path = (FilePath { file_path: path }).canonicalize().file_path;
            let prefix = match prefix {
                FilePrefix::Here | FilePrefix::Parent => ".",
                FilePrefix::Absolute => "/",
                FilePrefix::Home => "~",
            };
            let path =
                Some(prefix.to_string()).into_iter().chain(path).collect();
            ImportLocationKind::Local(path)
        }
        #[cfg(not(target_arch = "wasm32"))]
        ImportLocationKind::Remote(url) => {
            let mut url = url.clone();
            match prefix {
                FilePrefix::Here => {}
                FilePrefix::Parent => {
                    url = url.join("..")?;
                }
                FilePrefix::Absolute => panic!("error"),
                FilePrefix::Home => panic!("error"),
            }
            url = url.join(&path.file_path.join("/"))?;
            ImportLocationKind::Remote(url)
        }
        ImportLocationKind::NoImport => unreachable!(),
    })
}

#[cfg(not(feature = "std"))]
fn chain_local(
    _base: &ImportLocationKind,
    _prefix: FilePrefix,
    _path: &FilePath,
) -> Result<ImportLocationKind, Error> {
    Err(ImportError::Missing.into())
}

#[cfg(feature = "std")]
fn resolve_home(path: impl AsRef<std::path::Path>) -> Result<PathBuf, Error> {
    let mut f = PathBuf::new();
    match path.as_ref().strip_prefix("~") {
        Ok(rest) => {
            let home = home::home_dir()
                .ok_or_else(|| Error::from(ImportError::MissingHome))?;
            f.push(home);
            f.push(rest);
        }
        Err(_) => {
            f.push(path);
        }
    }
    Ok(f)
}

#[cfg(feature = "std")]
fn fetch_text(kind: &ImportLocationKind) -> Result<String, Error> {
    Ok(match kind {
        ImportLocationKind::Local(path) => {
            let path = resolve_home(path)?;
            std::fs::read_to_string(path)?
        }
        #[cfg(not(target_arch = "wasm32"))]
        ImportLocationKind::Remote(url) => download_http_text(url.clone())?,
        ImportLocationKind::Env(var_name) => match std::env::var(var_name) {
            Ok(val) => val,
            Err(_) => return Err(ImportError::MissingEnvVar.into()),
        },
        ImportLocationKind::Missing => {
            return Err(ImportError::Missing.into())
        }
        ImportLocationKind::NoImport => unreachable!(),
    })
}

#[cfg(not(feature = "std"))]
fn fetch_text(_kind: &ImportLocationKind) -> Result<String, Error> {
    Err(ImportError::Missing.into())
}

#[cfg(all(not(target_arch = "wasm32"), feature = "std", feature = "reqwest"))]
fn download_http_text(url: Url) -> Result<String, Error> {
    Ok(reqwest::blocking::get(url).unwrap().text().unwrap())
}

#[cfg(all(not(target_arch = "wasm32"), feature = "std", not(feature = "reqwest")))]
fn download_http_text(_url: Url) -> Result<String, Error> {
    panic!("Remote imports are disabled in this build of dhall-rust")
}
