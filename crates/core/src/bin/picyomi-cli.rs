//! Developer CLI: import dictionaries, debug lookups and OCR without the GUI.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use picyomi_core::{Db, dict, lookup, models, ocr};

fn usage() -> ! {
    eprintln!(
        "usage:
  picyomi-cli import-dict [--disabled] <zip>...
  picyomi-cli dicts [--enable ID] [--disable ID]
  picyomi-cli lookup <text> [offset]
  picyomi-cli ocr <image>..."
    );
    std::process::exit(2)
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first() else { usage() };
    let db = Db::open_default()?;
    match cmd.as_str() {
        "import-dict" => {
            let disabled = args.iter().any(|a| a == "--disabled");
            for path in args[1..].iter().filter(|a| !a.starts_with("--")) {
                let t = Instant::now();
                let r = dict::import_zip(&db, &PathBuf::from(path), !disabled, |_, _, _| {})?;
                println!("imported {}: {} terms, {} freq rows in {:.1}s", r.title, r.terms, r.meta, t.elapsed().as_secs_f32());
            }
        }
        "dicts" => {
            let mut it = args[1..].iter();
            while let Some(a) = it.next() {
                let id: i64 = it.next().map(|v| v.parse()).transpose()?.unwrap_or_else(|| usage());
                match a.as_str() {
                    "--enable" => dict::set_enabled(&db, id, true)?,
                    "--disable" => dict::set_enabled(&db, id, false)?,
                    _ => usage(),
                }
            }
            for d in dict::list(&db)? {
                println!(
                    "[{}] {} prio={} {}  terms={} freq={}",
                    d.id,
                    if d.enabled { "on " } else { "off" },
                    d.priority,
                    d.title,
                    d.term_count,
                    d.meta_count
                );
            }
        }
        "lookup" => {
            let Some(text) = args.get(1) else { usage() };
            let offset: usize = args.get(2).map(|v| v.parse()).transpose()?.unwrap_or(0);
            let t = Instant::now();
            let r = lookup::lookup(&db, text, offset, &[])?;
            let dt = t.elapsed();
            for e in &r.entries {
                let freq: Vec<_> = e.frequencies.iter().filter_map(|f| f.display.clone()).collect();
                println!("{} → {}【{}】 {} {:?} freq={}", e.source, e.expression, e.reading, e.dict, e.reasons, freq.join(","));
            }
            println!("({} entries in {:.1} ms)", r.entries.len(), dt.as_secs_f64() * 1000.0);
        }
        "serve" => {
            let port: u16 = args.get(1).map(|v| v.parse()).transpose()?.unwrap_or(picyomi_core::server::DEFAULT_PORT);
            let svc = std::sync::Arc::new(picyomi_core::service::Service::with_db(db));
            let rt = tokio::runtime::Runtime::new()?;
            println!("serving on http://127.0.0.1:{port}");
            rt.block_on(picyomi_core::server::serve(svc, port))?;
        }
        "set-llm" => {
            // set-llm <base_url> <model>; the key is read from $LLM_API_KEY so it never lands in shell history.
            let (Some(base_url), Some(model)) = (args.get(1), args.get(2)) else { usage() };
            let cfg = picyomi_core::explain::LlmConfig {
                base_url: base_url.clone(),
                model: model.clone(),
                api_key: std::env::var("LLM_API_KEY").unwrap_or_default(),
            };
            cfg.save(&db)?;
            println!("saved: {:?}", cfg.redacted());
        }
        "explain" => {
            let (Some(expression), Some(sentence)) = (args.get(1), args.get(2)) else { usage() };
            let req = picyomi_core::explain::ExplainRequest {
                expression: expression.clone(),
                reading: args.get(3).cloned().unwrap_or_default(),
                sentence: sentence.clone(),
                definition_hint: String::new(),
            };
            let t = Instant::now();
            let rt = tokio::runtime::Runtime::new()?;
            let e = rt.block_on(picyomi_core::explain::explain(&db, &req))?;
            println!("{}\n({:.1}s)", serde_json::to_string_pretty(&e)?, t.elapsed().as_secs_f32());
        }
        "install-models" => {
            // Developer convenience: install already-downloaded files (checksums verified).
            let Some(dir) = args.get(1) else { usage() };
            let dir = PathBuf::from(dir);
            models::install_local(&models::DETECTOR, &dir.join("ogkalu/detector-v4-s_int8.onnx"))?;
            models::install_local(&models::RECOGNIZER, &dir.join("charspot-small/model.onnx"))?;
            models::install_local(&models::RECOGNIZER_VOCAB, &dir.join("charspot-small/vocab.json"))?;
            println!("installed into {}", models::models_dir().display());
        }
        "ocr" => {
            let t = Instant::now();
            let engine = ocr::OcrEngine::load()?;
            println!("models loaded in {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
            for path in &args[1..] {
                let img = ocr::decode(&std::fs::read(path)?)?;
                let t = Instant::now();
                let page = engine.read_page(&img)?;
                println!("== {path}: {} blocks in {:.0} ms", page.blocks.len(), t.elapsed().as_secs_f64() * 1000.0);
                for b in &page.blocks {
                    println!(
                        "  [{:?}{} {:.2} {:?}] {}",
                        b.kind,
                        if b.vertical { " v" } else { " h" },
                        b.score,
                        b.bbox.map(|v| v as i32),
                        b.text
                    );
                }
            }
        }
        "audio" => {
            // audio <word> <reading> [sentence] [out]
            let e = args.get(1).context("usage: audio <word> <reading> [sentence] [out]")?;
            let r = args.get(2).map(|s| s.as_str()).unwrap_or("");
            let sentence = args.get(3).map(|s| s.as_str()).unwrap_or("");
            let db = picyomi_core::Db::open_default()?;
            let rt = tokio::runtime::Runtime::new()?;
            let t = Instant::now();
            let a = if sentence.is_empty() {
                rt.block_on(picyomi_core::audio::word(&db, e, r))?
            } else {
                rt.block_on(picyomi_core::audio::sentence(&db, sentence, e, r))?
            };
            println!("{} · {} · {} bytes · {:.0} ms · {}", a.source, a.mime, a.bytes.len(), t.elapsed().as_secs_f64() * 1000.0, a.path.display());
            if let Some(out) = args.get(4) {
                std::fs::write(out, &a.bytes)?;
            }
        }
        "import-anki" => {
            // import-anki <collection.anki2> [deck]
            let path = args.get(1).context("usage: import-anki <collection.anki2> [deck]")?;
            let svc = picyomi_core::service::Service::open()?;
            let t = Instant::now();
            let report = picyomi_core::anki::import(&svc, std::path::Path::new(path), args.get(2).map(|s| s.as_str()))?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            println!("done in {:.1} s", t.elapsed().as_secs_f64());
        }
        "ocr-screen" => {
            for path in &args[1..] {
                let img = ocr::decode(&std::fs::read(path)?)?;
                let t = Instant::now();
                let page = picyomi_core::vision::read_page(&img)?;
                println!("== {path}: {} blocks in {:.0} ms", page.blocks.len(), t.elapsed().as_secs_f64() * 1000.0);
                for b in &page.blocks {
                    println!("  [{:.2} {:?}] {}", b.score, b.bbox.map(|v| v as i32), b.text.replace('\n', "⏎"));
                }
            }
        }
        _ => bail!("unknown command {cmd}"),
    }
    Ok(())
}
