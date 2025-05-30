use std::path::{Path, PathBuf};

use anyhow::{format_err, Error};

use crate::repositories::release::DebianCodename;
use proxmox_apt_api_types::{
    APTRepository, APTRepositoryFile, APTRepositoryFileError, APTRepositoryFileType,
    APTRepositoryInfo, APTRepositoryPackageType,
};

use crate::repositories::repository::APTRepositoryImpl;

mod list_parser;
use list_parser::APTListFileParser;

mod sources_parser;
use sources_parser::APTSourcesFileParser;

use proxmox_config_digest::ConfigDigest;

trait APTRepositoryParser {
    /// Parse all repositories including the disabled ones and push them onto
    /// the provided vector.
    fn parse_repositories(&mut self) -> Result<Vec<APTRepository>, Error>;
}

pub trait APTRepositoryFileImpl {
    /// Creates a new `APTRepositoryFile` without parsing.
    ///
    /// If the file is hidden, the path points to a directory, or the extension
    /// is usually ignored by APT (e.g. `.orig`), `Ok(None)` is returned, while
    /// invalid file names yield an error.
    #[allow(clippy::new_ret_no_self)]
    fn new<P: AsRef<Path>>(path: P) -> Result<Option<APTRepositoryFile>, APTRepositoryFileError>;

    fn with_content(content: String, content_type: APTRepositoryFileType) -> Self;

    /// Check if the file exists.
    fn exists(&self) -> bool;

    fn read_with_digest(&self) -> Result<(Vec<u8>, ConfigDigest), APTRepositoryFileError>;

    /// Create an `APTRepositoryFileError`.
    fn err(&self, error: Error) -> APTRepositoryFileError;

    /// Parses the APT repositories configured in the file on disk, including
    /// disabled ones.
    ///
    /// Resets the current repositories and digest, even on failure.
    fn parse(&mut self) -> Result<(), APTRepositoryFileError>;

    /// Writes the repositories to the file on disk.
    ///
    /// If a digest is provided, checks that the current content of the file still
    /// produces the same one.
    fn write(&self) -> Result<(), APTRepositoryFileError>;

    /// Checks if old or unstable suites are configured and that the Debian security repository
    /// has the correct suite. Also checks that the `stable` keyword is not used.
    fn check_suites(&self, current_codename: DebianCodename) -> Vec<APTRepositoryInfo>;

    /// Checks for official URIs.
    fn check_uris(&self, apt_lists_dir: &Path) -> Vec<APTRepositoryInfo>;
}

impl APTRepositoryFileImpl for APTRepositoryFile {
    fn new<P: AsRef<Path>>(path: P) -> Result<Option<Self>, APTRepositoryFileError> {
        let path: PathBuf = path.as_ref().to_path_buf();

        let new_err = |path_string: String, err: &str| APTRepositoryFileError {
            path: path_string,
            error: err.to_string(),
        };

        let path_string = path
            .clone()
            .into_os_string()
            .into_string()
            .map_err(|os_string| {
                new_err(
                    os_string.to_string_lossy().to_string(),
                    "path is not valid unicode",
                )
            })?;

        let new_err = |err| new_err(path_string.clone(), err);

        if path.is_dir() {
            return Ok(None);
        }

        let file_name = match path.file_name() {
            Some(file_name) => file_name
                .to_os_string()
                .into_string()
                .map_err(|_| new_err("invalid path"))?,
            None => return Err(new_err("invalid path")),
        };

        if file_name.starts_with('.') || file_name.ends_with('~') {
            return Ok(None);
        }

        let extension = match path.extension() {
            Some(extension) => extension
                .to_os_string()
                .into_string()
                .map_err(|_| new_err("invalid path"))?,
            None => return Err(new_err("invalid extension")),
        };

        // See APT's apt-pkg/init.cc
        if extension.starts_with("dpkg-")
            || extension.starts_with("ucf-")
            || matches!(
                extension.as_str(),
                "disabled" | "bak" | "save" | "orig" | "distUpgrade"
            )
        {
            return Ok(None);
        }

        let file_type = extension[..]
            .parse()
            .map_err(|_| new_err("invalid extension"))?;

        if !file_name
            .chars()
            .all(|x| x.is_ascii_alphanumeric() || x == '_' || x == '-' || x == '.')
        {
            return Err(new_err("invalid characters in file name"));
        }

        Ok(Some(Self {
            path: Some(path_string),
            file_type,
            repositories: vec![],
            digest: None,
            content: None,
        }))
    }

    fn with_content(content: String, content_type: APTRepositoryFileType) -> Self {
        Self {
            file_type: content_type,
            content: Some(content),
            path: None,
            repositories: vec![],
            digest: None,
        }
    }

    fn exists(&self) -> bool {
        if let Some(path) = &self.path {
            PathBuf::from(path).exists()
        } else {
            false
        }
    }

    fn read_with_digest(&self) -> Result<(Vec<u8>, ConfigDigest), APTRepositoryFileError> {
        if let Some(path) = &self.path {
            let content = std::fs::read(path).map_err(|err| self.err(format_err!("{}", err)))?;
            let digest = ConfigDigest::from_slice(&content);

            Ok((content, digest))
        } else if let Some(ref content) = self.content {
            let content = content.as_bytes();
            let digest = ConfigDigest::from_slice(content);
            Ok((content.to_vec(), digest))
        } else {
            Err(self.err(format_err!(
                "Neither 'path' nor 'content' set, cannot read APT repository info."
            )))
        }
    }

    fn err(&self, error: Error) -> APTRepositoryFileError {
        APTRepositoryFileError {
            path: self.path.clone().unwrap_or_default(),
            error: error.to_string(),
        }
    }

    fn parse(&mut self) -> Result<(), APTRepositoryFileError> {
        self.repositories.clear();
        self.digest = None;

        let (content, digest) = self.read_with_digest()?;

        let mut parser: Box<dyn APTRepositoryParser> = match self.file_type {
            APTRepositoryFileType::List => Box::new(APTListFileParser::new(&content[..])),
            APTRepositoryFileType::Sources => Box::new(APTSourcesFileParser::new(&content[..])),
        };

        let repos = parser.parse_repositories().map_err(|err| self.err(err))?;

        for (n, repo) in repos.iter().enumerate() {
            repo.basic_check()
                .map_err(|err| self.err(format_err!("check for repository {} - {err}", n + 1)))?;
        }

        self.repositories = repos;
        self.digest = Some(*digest);

        Ok(())
    }

    fn write(&self) -> Result<(), APTRepositoryFileError> {
        let path = match &self.path {
            Some(path) => path,
            None => {
                return Err(self.err(format_err!(
                    "Cannot write to APT repository file without path."
                )));
            }
        };

        if let Some(digest) = &self.digest {
            if !self.exists() {
                return Err(self.err(format_err!("digest specified, but file does not exist")));
            }

            let (_, current_digest) = self.read_with_digest()?;
            if *digest != *current_digest {
                return Err(self.err(format_err!("digest mismatch")));
            }
        }

        if self.repositories.is_empty() {
            return std::fs::remove_file(path)
                .map_err(|err| self.err(format_err!("unable to remove file - {err}")));
        }

        use std::io::Write;
        let mut content = vec![];

        for (n, repo) in self.repositories.iter().enumerate() {
            let entry = n + 1;
            repo.basic_check()
                .map_err(|err| self.err(format_err!("check for repository {entry} - {err}")))?;

            if !content.is_empty() {
                writeln!(content).map_err(|err| {
                    self.err(format_err!("internal error for repository {entry} - {err}",))
                })?;
            }
            repo.write(&mut content)
                .map_err(|err| self.err(format_err!("writing repository {entry} - {err}")))?;
        }

        let path = PathBuf::from(&path);
        let dir = match path.parent() {
            Some(dir) => dir,
            None => return Err(self.err(format_err!("invalid path"))),
        };

        std::fs::create_dir_all(dir)
            .map_err(|err| self.err(format_err!("unable to create parent dir - {err}")))?;

        let pid = std::process::id();
        let mut tmp_path = path.clone();
        tmp_path.set_extension("tmp");
        tmp_path.set_extension(format!("{}", pid));

        if let Err(err) = std::fs::write(&tmp_path, content) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(self.err(format_err!("writing {path:?} failed - {err}")));
        }

        if let Err(err) = std::fs::rename(&tmp_path, &path) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(self.err(format_err!("rename failed for {path:?} - {err}")));
        }

        Ok(())
    }

    fn check_suites(&self, current_codename: DebianCodename) -> Vec<APTRepositoryInfo> {
        let mut infos = vec![];

        let path = match &self.path {
            Some(path) => path.clone(),
            None => return vec![],
        };

        for (n, repo) in self.repositories.iter().enumerate() {
            if !repo.types.contains(&APTRepositoryPackageType::Deb) {
                continue;
            }

            let is_security_repo = repo.uris.iter().any(|uri| {
                let uri = uri.trim_end_matches('/');
                let uri = uri.strip_suffix("debian-security").unwrap_or(uri);
                let uri = uri.trim_end_matches('/');
                matches!(
                    uri,
                    "http://security.debian.org" | "https://security.debian.org",
                )
            });

            let require_suffix = match is_security_repo {
                true if current_codename >= DebianCodename::Bullseye => Some("-security"),
                true => Some("/updates"),
                false => None,
            };

            let mut add_info = |kind: &str, message| {
                infos.push(APTRepositoryInfo {
                    path: path.clone(),
                    index: n,
                    property: Some("Suites".to_string()),
                    kind: kind.to_string(),
                    message,
                })
            };

            let message_old = |suite| format!("old suite '{}' configured!", suite);
            let message_new =
                |suite| format!("suite '{}' should not be used in production!", suite);
            let message_stable = "use the name of the stable distribution instead of 'stable'!";

            for suite in repo.suites.iter() {
                let (base_suite, suffix) = suite_variant(suite);

                match base_suite {
                    "oldoldstable" | "oldstable" => {
                        add_info("warning", message_old(base_suite));
                    }
                    "testing" | "unstable" | "experimental" | "sid" => {
                        add_info("warning", message_new(base_suite));
                    }
                    "stable" => {
                        add_info("warning", message_stable.to_string());
                    }
                    _ => (),
                };

                let codename: DebianCodename = match base_suite.try_into() {
                    Ok(codename) => codename,
                    Err(_) => continue,
                };

                if codename < current_codename {
                    add_info("warning", message_old(base_suite));
                }

                if Some(codename) == current_codename.next() {
                    add_info("ignore-pre-upgrade-warning", message_new(base_suite));
                } else if codename > current_codename {
                    add_info("warning", message_new(base_suite));
                }

                if let Some(require_suffix) = require_suffix {
                    if suffix != require_suffix {
                        add_info(
                            "warning",
                            format!("expected suite '{}{}'", current_codename, require_suffix),
                        );
                    }
                }
            }
        }

        infos
    }

    fn check_uris(&self, apt_lists_dir: &Path) -> Vec<APTRepositoryInfo> {
        let mut infos = vec![];

        let path = match &self.path {
            Some(path) => path,
            None => return vec![],
        };

        for (n, repo) in self.repositories.iter().enumerate() {
            let mut origin = repo.get_cached_origin(apt_lists_dir).unwrap_or_default();

            if origin.is_none() {
                origin = repo.origin_from_uris();
            }

            if let Some(origin) = origin {
                infos.push(APTRepositoryInfo {
                    path: path.clone(),
                    index: n,
                    kind: "origin".to_string(),
                    property: None,
                    message: origin,
                });
            }
        }

        infos
    }
}

/// Splits the suite into its base part and variant.
/// Does not expect the base part to contain either `-` or `/`.
fn suite_variant(suite: &str) -> (&str, &str) {
    match suite.find(&['-', '/'][..]) {
        Some(n) => (&suite[0..n], &suite[n..]),
        None => (suite, ""),
    }
}
