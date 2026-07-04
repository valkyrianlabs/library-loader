// * Keep these in alphabetical order
pub mod kicad;

use std::io::Cursor;
pub(super) use {
    std::{collections::HashMap, io::Read, path::PathBuf},
    {super::Format, crate::error::Result},
};

pub type Files = HashMap<String, Vec<u8>>;
pub const MAX_EXTRACTED_FILE_BYTES: u64 = 64 * 1024 * 1024;

pub(super) fn generic_extractor(
    format: &Format,
    archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
) -> Result<HashMap<String, Vec<u8>>> {
    let mut files = Files::new();
    'file_loop: for i in 0..archive.len() {
        let mut item = archive.by_index(i)?;
        if item.is_dir() {
            continue;
        }

        let file_path = item.name().to_string();
        let file_path_lower = file_path.to_lowercase();

        // Ignore files
        for ignore in &format.ignore {
            if file_path_lower.contains(ignore.to_lowercase().as_str()) {
                continue 'file_loop;
            }
        }

        if file_path_lower.contains(&format.match_path.to_lowercase()) {
            let path = PathBuf::from(file_path);
            let Some(base_name) = path.file_name() else {
                continue;
            };
            let base_name = base_name.to_string_lossy().to_string();
            let mut f_data = Vec::<u8>::new();
            item.by_ref()
                .take(MAX_EXTRACTED_FILE_BYTES + 1)
                .read_to_end(&mut f_data)?;
            if f_data.len() as u64 > MAX_EXTRACTED_FILE_BYTES {
                return Err(crate::error::Error::ZipEntryTooLarge(
                    base_name,
                    f_data.len() as u64,
                ));
            }
            files.insert(base_name, f_data);
        }
    }

    Ok(files)
}
