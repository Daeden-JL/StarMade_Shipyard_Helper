//! A zip holding a model and the files that belong to it.

use std::io::Read;

use crate::error::{malformed, too_large, ImportError, Result};
use crate::files::{extension, Files, Source};
use crate::model::Model;

/// The most bytes that are unpacked from one upload, whatever the zip
/// claims to hold. A zip of a few kilobytes can unpack to terabytes.
pub(crate) const MAX_UNPACKED: u64 = 512 * 1024 * 1024;
const MAX_ENTRIES: usize = 20_000;

/// The endings of the files a model may bring along.
const COMPANIONS: [&str; 5] = ["mtl", "bin", "png", "jpg", "jpeg"];

pub(crate) type Zip<'a> = zip::ZipArchive<std::io::Cursor<&'a [u8]>>;

pub(crate) fn open(bytes: &[u8]) -> Result<Zip<'_>> {
    let archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|_| malformed("The zip file is cut short or damaged."))?;
    if archive.len() > MAX_ENTRIES {
        return Err(too_large(format!("The zip holds more than {MAX_ENTRIES} files.")));
    }
    Ok(archive)
}

/// Unpacks one entry, taking its size off what may still be unpacked.
pub(crate) fn unpack(archive: &mut Zip, index: usize, budget: &mut u64) -> Result<Vec<u8>> {
    let entry = archive.by_index(index).map_err(|_| malformed("A file in the zip is damaged or locked with a password."))?;
    let mut bytes = Vec::with_capacity(entry.size().min(*budget).min(64 * 1024 * 1024) as usize);
    entry.take(*budget + 1).read_to_end(&mut bytes).map_err(|_| malformed("A file in the zip is cut short or damaged."))?;
    if bytes.len() as u64 > *budget {
        return Err(too_large(format!("The zip unpacks to more than {} MB.", MAX_UNPACKED / (1024 * 1024))));
    }
    *budget -= bytes.len() as u64;
    Ok(bytes)
}

/// The names and unpacked sizes of the real files in a zip: no folders,
/// none of the shadow files macOS adds.
pub(crate) fn listing(archive: &mut Zip) -> Vec<(usize, String, u64)> {
    let mut out = Vec::new();
    for index in 0..archive.len() {
        let Ok(entry) = archive.by_index_raw(index) else { continue };
        let name = entry.name().to_string();
        let bare = name.rsplit('/').next().unwrap_or("");
        if entry.is_dir() || name.starts_with("__MACOSX/") || bare.starts_with("._") || bare.is_empty() {
            continue;
        }
        out.push((index, name, entry.size()));
    }
    out
}

/// Reads the model in a zip: the 3MF or the one model file in it, or the
/// largest of several, with the materials, buffers and pictures beside it.
pub(crate) fn read(source: Source) -> Result<Model> {
    let mut archive = open(source.bytes)?;
    let entries = listing(&mut archive);
    if entries.iter().any(|(_, name, _)| extension(name) == "model") {
        return crate::threemf::read(&mut archive, &entries);
    }
    let models: Vec<&(usize, String, u64)> = entries.iter().filter(|(_, name, _)| crate::kind_by_extension(&extension(name)).is_some_and(|kind| kind != crate::Kind::Zip)).collect();
    let Some(chosen) = models.iter().max_by_key(|(_, _, size)| *size) else {
        // Say what is the matter with a file that is known but unreadable.
        if let Some(message) = entries.iter().find_map(|(_, name, _)| crate::unreadable(&extension(name))) {
            return Err(ImportError::Unsupported(format!("The zip holds {}. {message}", entries.iter().find(|(_, name, _)| crate::unreadable(&extension(name)).is_some()).map_or("a file", |(_, name, _)| name.as_str()))));
        }
        return Err(ImportError::UnknownFormat(format!("The zip holds no model file. It should hold one of: {}.", crate::extension_list())));
    };
    let mut budget = MAX_UNPACKED;
    let bytes = unpack(&mut archive, chosen.0, &mut budget)?;
    let mut files = Files::new();
    for (index, name, _) in entries.iter().filter(|(_, name, _)| COMPANIONS.contains(&extension(name).as_str())) {
        files.add(name, unpack(&mut archive, *index, &mut budget)?);
    }
    let mut model = crate::read_source(Source { name: &chosen.1, bytes: &bytes, files: &files }, false)?;
    if models.len() > 1 {
        model.notes.insert(0, format!("The zip holds {} models; the largest, {}, was used.", models.len(), chosen.1));
    }
    Ok(model)
}
