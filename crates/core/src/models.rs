//! OCR model files: pinned revisions, checksums, first-run download.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

use crate::db::data_dir;

pub struct ModelFile {
    pub name: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub license: &'static str,
}

/// RT-DETR v4-s (int8): page-level detector for bubbles, text in bubbles and free text. Apache-2.0.
pub const DETECTOR: ModelFile = ModelFile {
    name: "detector-v4-s_int8.onnx",
    url: "https://huggingface.co/ogkalu/comic-text-and-bubble-detector/resolve/16e8a622f91fabc6b5b65c96d32d1183f8843546/detector-v4-s_int8.onnx",
    sha256: "5fe9e4f576e49d4e7e8b0e029d6d3cdc252abd4694113e1cae120e62c931ea79",
    license: "Apache-2.0 (ogkalu/comic-text-and-bubble-detector)",
};

/// manga-ocr-charspot-small: per-character detection + reading order. CC BY-NC-SA 4.0 (non-commercial).
pub const RECOGNIZER: ModelFile = ModelFile {
    name: "charspot-small.onnx",
    url: "https://huggingface.co/muscgab/manga-ocr-charspot-small/resolve/72ca29202673880ca9c67b1c02c107535be4472b/model.onnx",
    sha256: "05fb79760f8e3dbc989b61f23a34c46b511818a2166573ce7e41d994f4403843",
    license: "CC BY-NC-SA 4.0 (muscgab/manga-ocr-charspot-small)",
};

pub const RECOGNIZER_VOCAB: ModelFile = ModelFile {
    name: "charspot-small.vocab.json",
    url: "https://huggingface.co/muscgab/manga-ocr-charspot-small/resolve/72ca29202673880ca9c67b1c02c107535be4472b/vocab.json",
    sha256: "9f72fc7852b1432bcb61b3b94b2e4cd8abef52b01e9051550a6a3070e2b314d5",
    license: "CC BY-NC-SA 4.0 (muscgab/manga-ocr-charspot-small)",
};

pub const ALL: [&ModelFile; 3] = [&DETECTOR, &RECOGNIZER, &RECOGNIZER_VOCAB];

pub fn models_dir() -> PathBuf {
    data_dir().join("models")
}

pub fn path_of(m: &ModelFile) -> PathBuf {
    models_dir().join(m.name)
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h)?;
    Ok(format!("{:x}", h.finalize()))
}

pub fn is_present(m: &ModelFile) -> bool {
    path_of(m).is_file()
}

pub fn all_present() -> bool {
    ALL.iter().all(|m| is_present(m))
}

/// Download every missing model file, verifying its checksum.
/// `progress(name, downloaded_bytes, total_bytes)`.
pub async fn ensure_all(mut progress: impl FnMut(&str, u64, Option<u64>)) -> Result<()> {
    let client = reqwest::Client::builder().user_agent("pic_yomi").build()?;
    for m in ALL {
        let dest = path_of(m);
        if dest.is_file() {
            continue;
        }
        let tmp = dest.with_extension("part");
        let mut resp = client.get(m.url).send().await?.error_for_status().with_context(|| format!("download {}", m.name))?;
        let total = resp.content_length();
        let mut file = std::fs::File::create(&tmp)?;
        let mut got = 0u64;
        while let Some(chunk) = resp.chunk().await? {
            file.write_all(&chunk)?;
            got += chunk.len() as u64;
            progress(m.name, got, total);
        }
        file.flush()?;
        drop(file);
        let sum = sha256_file(&tmp)?;
        if sum != m.sha256 {
            let _ = std::fs::remove_file(&tmp);
            bail!("{}: checksum mismatch (got {sum})", m.name);
        }
        std::fs::rename(&tmp, &dest)?;
    }
    Ok(())
}

/// Copy already-downloaded files into place (developer convenience), verifying checksums.
pub fn install_local(m: &ModelFile, src: &Path) -> Result<()> {
    let sum = sha256_file(src)?;
    if sum != m.sha256 {
        bail!("{}: checksum mismatch for {}", m.name, src.display());
    }
    std::fs::copy(src, path_of(m))?;
    Ok(())
}
