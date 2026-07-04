use {
    crate::error::{self, Error},
    std::{
        collections::HashMap,
        fs,
        path::{Component, Path, PathBuf},
    },
};

pub struct Result {
    pub output_path: PathBuf,
    pub files: HashMap<String, Vec<u8>>,
}

impl Result {
    pub fn save(&self) -> error::Result<PathBuf> {
        let save_dir = Path::new(&self.output_path);

        if !self.files.is_empty() {
            if !save_dir.exists() {
                fs::create_dir_all(save_dir)?;
            }

            for (filename, data) in &self.files {
                let path = self.output_file_path(filename)?;
                Self::write(path, data.to_vec())?;
            }

            Ok(save_dir.canonicalize()?)
        } else {
            // Err(new_err!("No files found for your specified library"))
            Err(Error::NoFilesInLibrary)
        }
    }

    fn output_file_path(&self, filename: &str) -> error::Result<PathBuf> {
        let path = PathBuf::from(filename);
        if path.is_absolute() {
            return Ok(path);
        }

        if path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            return Err(Error::Other("Refusing to write outside output directory"));
        }

        Ok(Path::new(&self.output_path).join(path))
    }

    fn write(path: PathBuf, data: Vec<u8>) -> error::Result<PathBuf> {
        if path.exists() {
            if fs::read(&path)? == data {
                return Ok(path);
            }

            return Err(Error::WouldOverwrite);
        }

        if let Some(parent) = path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent)?;
            }
        }

        fs::write(&path, data)?;
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn save_creates_parent_dirs_and_skips_identical_files() {
        let output_path = temp_dir("save");
        let mut files = HashMap::new();
        files.insert("nested/model.stp".to_owned(), b"model".to_vec());
        let result = Result {
            output_path: output_path.clone(),
            files,
        };

        result.save().expect("first save");
        result.save().expect("second identical save");
        assert_eq!(
            fs::read(output_path.join("nested/model.stp")).expect("saved file"),
            b"model"
        );
    }

    #[test]
    fn save_rejects_relative_parent_paths() {
        let output_path = temp_dir("reject");
        let mut files = HashMap::new();
        files.insert("../escape.stp".to_owned(), b"model".to_vec());
        let result = Result { output_path, files };

        assert!(matches!(result.save(), Err(Error::Other(_))));
    }

    fn temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "library-loader-result-{}-{}-{}",
            name,
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&path).expect("create temp dir");
        path
    }
}
