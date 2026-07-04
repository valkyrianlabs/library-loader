use super::*;
use std::{collections::HashMap, fs};

pub fn extract(
    format: &Format,
    archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
) -> Result<HashMap<String, Vec<u8>>> {
    let fp_folder_str = format!("{}.pretty", format.name);
    let mut files = Files::new();
    let mut symbols = Vec::<String>::new();
    let model_uris = collect_model_uris(format, archive)?;

    for i in 0..archive.len() {
        let mut item = archive.by_index(i)?;
        if item.is_dir() {
            continue;
        }

        let name = item.name().to_owned();
        let path = PathBuf::from(&name);
        let Some(base_name) = path.file_name() else {
            continue;
        };
        let base_name = base_name.to_string_lossy().to_string();

        if let Some(ext) = &path.extension() {
            match ext.to_str().map(|ext| ext.to_ascii_lowercase()).as_deref() {
                Some("kicad_mod") => {
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

                    let f_data = rewrite_model_paths(f_data, &model_uris);
                    files.insert(format!("{}/{}", fp_folder_str, base_name), f_data);
                }
                Some(ext)
                    if format
                        .model_extensions
                        .iter()
                        .any(|model_ext| model_ext == ext) =>
                {
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

                    files.insert(model_output_key(format, &fp_folder_str, &base_name), f_data);
                }
                Some("kicad_sym") => {
                    symbols.push(name);
                }
                Some("lib") | Some("dcm") => {
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

                    files.insert(format!("{}.legacy/{}", format.name, base_name), f_data);
                }
                _ => {
                    // ignore all other files
                }
            }
        }
    }

    merge_symbol_libraries(format, archive, symbols)?;

    Ok(files)
}

fn collect_model_uris(
    format: &Format,
    archive: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>,
) -> Result<HashMap<String, String>> {
    let mut models = HashMap::new();

    for i in 0..archive.len() {
        let item = archive.by_index(i)?;
        if item.is_dir() {
            continue;
        }

        let path = PathBuf::from(item.name());
        let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
            continue;
        };
        let ext = ext.to_ascii_lowercase();
        if !format
            .model_extensions
            .iter()
            .any(|model_ext| model_ext == &ext)
        {
            continue;
        }

        let Some(base_name) = path.file_name() else {
            continue;
        };
        let base_name = base_name.to_string_lossy().to_string();
        let Some(model_uri) = &format.model_uri else {
            continue;
        };

        models.insert(
            base_name.clone(),
            format!("{}/{}", model_uri.trim_end_matches('/'), base_name),
        );
    }

    Ok(models)
}

fn model_output_key(format: &Format, fp_folder: &str, base_name: &str) -> String {
    match &format.model_output_path {
        Some(model_output_path) if model_output_path.is_absolute() => model_output_path
            .join(base_name)
            .to_string_lossy()
            .to_string(),
        Some(model_output_path) => model_output_path
            .join(base_name)
            .to_string_lossy()
            .to_string(),
        None => format!("{}/{}", fp_folder, base_name),
    }
}

fn rewrite_model_paths(data: Vec<u8>, model_uris: &HashMap<String, String>) -> Vec<u8> {
    if model_uris.is_empty() {
        return data;
    }

    let text = match String::from_utf8(data) {
        Ok(text) => text,
        Err(err) => return err.into_bytes(),
    };

    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        if let Some(rewritten) = rewrite_model_line(line, model_uris) {
            out.push_str(&rewritten);
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }

    out.into_bytes()
}

fn rewrite_model_line(line: &str, model_uris: &HashMap<String, String>) -> Option<String> {
    let model_start = line.find("(model ")?;
    let value_start = model_start + "(model ".len();
    let rest = &line[value_start..];
    let trimmed = rest.trim_start();
    let leading_ws = rest.len() - trimmed.len();
    let value_start = value_start + leading_ws;

    let (value, value_end) = if let Some(stripped) = trimmed.strip_prefix('"') {
        let end = stripped.find('"')?;
        (&stripped[..end], value_start + 1 + end + 1)
    } else {
        let end = trimmed
            .find(|ch: char| ch.is_whitespace() || ch == ')')
            .unwrap_or(trimmed.len());
        (&trimmed[..end], value_start + end)
    };

    let base_name = PathBuf::from(value)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())?;
    let uri = model_uris.get(&base_name)?;
    let mut rewritten = String::with_capacity(line.len() + uri.len());
    rewritten.push_str(&line[..value_start]);
    rewritten.push('"');
    rewritten.push_str(uri);
    rewritten.push('"');
    rewritten.push_str(&line[value_end..]);
    Some(rewritten)
}

fn merge_symbol_libraries(
    format: &Format,
    archive: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>,
    symbols: Vec<String>,
) -> Result<()> {
    if symbols.is_empty() {
        return Ok(());
    }

    if !format.output_path.exists() {
        fs::create_dir_all(&format.output_path)?;
    }

    let fn_lib = format
        .output_path
        .join(format!("{}.kicad_sym", format.name));

    if !fn_lib.exists() {
        fs::write(
            &fn_lib,
            "(kicad_symbol_lib (version 20211014) (generator library-loader)\n)\n",
        )?;
    }

    let existing = fs::read_to_string(&fn_lib)?;
    let mut additions = Vec::<String>::new();

    for symbol_file in symbols {
        let mut f_data = Vec::<u8>::new();
        let mut item = archive.by_name(&symbol_file)?;
        item.by_ref()
            .take(MAX_EXTRACTED_FILE_BYTES + 1)
            .read_to_end(&mut f_data)?;
        if f_data.len() as u64 > MAX_EXTRACTED_FILE_BYTES {
            return Err(crate::error::Error::ZipEntryTooLarge(
                symbol_file,
                f_data.len() as u64,
            ));
        }

        let Ok(text) = String::from_utf8(f_data) else {
            continue;
        };
        let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
        if lines.len() < 2 {
            continue;
        }

        let end = lines.len() - 1;
        for line in lines.iter_mut().take(end) {
            let parts = line.split_whitespace().collect::<Vec<_>>();
            if parts.len() >= 2 && parts[0] == "(property" && parts[1] == "\"Footprint\"" {
                let footprint_name = &parts[2][1..(parts[2].len() - 1)];
                *line = line.replace(
                    footprint_name,
                    &format!("{}:{}", format.name, &footprint_name),
                );
            }
        }

        let body = lines[1..end].join("\n");
        if body.trim().is_empty() || symbol_already_present(&existing, &body) {
            continue;
        }

        additions.push(body);
    }

    if additions.is_empty() {
        return Ok(());
    }

    let mut merged = existing.trim_end().to_owned();
    if merged.ends_with(')') {
        merged.pop();
        merged = merged.trim_end().to_owned();
    }
    merged.push('\n');
    merged.push_str(&additions.join("\n"));
    merged.push_str("\n)\n");
    fs::write(fn_lib, merged)?;

    Ok(())
}

fn symbol_already_present(existing: &str, body: &str) -> bool {
    body.lines()
        .find_map(|line| {
            line.trim_start()
                .strip_prefix("(symbol \"")
                .and_then(|rest| rest.split('"').next())
        })
        .is_some_and(|name| existing.contains(&format!("(symbol \"{}\"", name)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::ECAD;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn extracts_models_to_configured_output_and_rewrites_footprint() {
        let fixture =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../test-files/LIB_ATMEGA328P-AU.zip");
        let data = fs::read(fixture).expect("fixture should exist");
        let mut archive =
            zip::ZipArchive::new(std::io::Cursor::new(data.as_slice())).expect("valid fixture");
        let output_dir = temp_dir("kicad-output");
        let model_dir = temp_dir("kicad-models");
        let mut format = Format::from_ecad(&"QuartzPulseLib".to_owned(), ECAD::KiCad, output_dir);
        format.model_output_path = Some(model_dir.clone());
        format.model_uri = Some("${QP_3DMODELS}".to_owned());

        let files = extract(&format, &mut archive).expect("extract fixture");
        let footprint_key = "QuartzPulseLib.pretty/QFP80P900X900X120-32N.kicad_mod";
        let footprint = String::from_utf8(files[footprint_key].clone()).expect("utf8 footprint");

        assert!(footprint.contains("\"${QP_3DMODELS}/ATMEGA328P-AU.stp\""));
        assert!(files.contains_key(
            model_dir
                .join("ATMEGA328P-AU.stp")
                .to_string_lossy()
                .as_ref()
        ));
        assert!(files.contains_key(
            model_dir
                .join("ATMEGA328P-AU.wrl")
                .to_string_lossy()
                .as_ref()
        ));
        assert!(files.contains_key("QuartzPulseLib.legacy/ATMEGA328P-AU.lib"));
    }

    fn temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "library-loader-{}-{}-{}",
            name,
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&path).expect("create temp dir");
        path
    }
}
