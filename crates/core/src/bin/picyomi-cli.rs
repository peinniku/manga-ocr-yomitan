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
        "set-reading" => {
            // set-reading <expression> <new reading>: fix a card saved with the wrong reading
            let (e, r) = (args.get(1).context("usage: set-reading <expression> <reading>")?, args.get(2).context("usage: set-reading <expression> <reading>")?);
            let svc = picyomi_core::service::Service::open()?;
            let ids: Vec<String> = {
                let c = svc.db.lock();
                let mut st = c.prepare("SELECT id FROM cards WHERE deleted=0 AND json_extract(data, '$.expression')=?1")?;
                st.query_map([e], |r| r.get(0))?.collect::<Result<_, _>>()?
            };
            let rt = tokio::runtime::Runtime::new()?;
            for id in ids {
                let card = svc.set_reading(&id, r)?;
                let filled = rt.block_on(svc.fill_explanation(&id))?;
                println!(
                    "{}【{}】 {} · {}",
                    card.expression,
                    card.reading,
                    card.frequency.unwrap_or_default(),
                    filled.and_then(|c| c.explanation).map(|x| x.meaning_zh).unwrap_or_else(|| "(no explanation)".into())
                );
            }
        }
        "eval-lookup" => {
            // eval-lookup: look each card's word up in its own sentence and check
            // whether the first entry is the card's word and reading.
            let svc = picyomi_core::service::Service::open()?;
            let rows: Vec<String> = {
                let c = svc.db.lock();
                let mut st = c.prepare("SELECT data FROM cards WHERE deleted=0")?;
                st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?
            };
            let (mut total, mut top1) = (0, 0);
            for d in rows {
                let card: picyomi_core::cards::Card = serde_json::from_str(&d)?;
                let Some(span) = picyomi_core::lookup::find_word(&card.sentence, &card.expression, &card.reading) else { continue };
                let offset = card.sentence[..span.start].chars().count();
                let res = svc.lookup(&card.sentence, offset)?;
                let want = picyomi_core::kana::kata_to_hira(&card.reading);
                let pos = res.result.entries.iter().position(|e| e.expression == card.expression && picyomi_core::kana::kata_to_hira(&e.reading) == want);
                total += 1;
                if pos == Some(0) {
                    top1 += 1;
                } else {
                    let first = res.result.entries.first().map(|e| format!("{}【{}】", e.expression, e.reading)).unwrap_or_default();
                    println!("MISS {}【{}】 rank {:?} · first {} · {}", card.expression, card.reading, pos.map(|p| p + 1), first, card.sentence.chars().take(36).collect::<String>());
                }
            }
            println!("top-1 {top1}/{total} = {:.1}%", 100.0 * top1 as f64 / total as f64);
        }
        "check-readings" => {
            // check-readings: let the LLM pick each ambiguous card's reading from its sentence
            let svc = picyomi_core::service::Service::open()?;
            let ids: Vec<(String, String)> = {
                let c = svc.db.lock();
                let mut st = c.prepare("SELECT id, json_extract(data, '$.expression') FROM cards WHERE deleted=0")?;
                st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?
            };
            let rt = tokio::runtime::Runtime::new()?;
            let (mut checked, mut changed) = (0, 0);
            for (id, expression) in ids {
                if picyomi_core::explain::readings_of(&svc.db, &expression).len() < 2 {
                    continue;
                }
                checked += 1;
                match rt.block_on(svc.recheck_reading(&id)) {
                    Ok(Some(old)) => {
                        changed += 1;
                        let card = picyomi_core::cards::get_card(&svc.db, &id)?.context("card")?;
                        println!("CHANGED {expression}: {old} → {} · {}", card.reading, card.sentence.chars().take(30).collect::<String>());
                    }
                    Ok(None) => {}
                    Err(e) => println!("ERROR {expression}: {e:#}"),
                }
            }
            println!("checked {checked}, changed {changed}");
        }
        "fill-explanations" => {
            // fill-explanations: add the Chinese explanation to every card that has none
            let svc = picyomi_core::service::Service::open()?;
            let rt = tokio::runtime::Runtime::new()?;
            println!("explained {} cards", rt.block_on(svc.fill_missing_explanations())?);
        }
        "cloud-setup" => {
            // cloud-setup <https://picyomi.xxx.workers.dev>: point this Mac at the hub
            let url = args.get(1).context("usage: cloud-setup <hub url>")?;
            let db = picyomi_core::Db::open_default()?;
            picyomi_core::cloud::configure(&db, url)?;
            println!("cloud hub set to {url} (token stored locally; set it on the hub with scripts/deploy-cloud.sh)");
        }
        "cloud-sync" => {
            // cloud-sync: one round now, with a report
            let svc = picyomi_core::service::Service::open()?;
            let rt = tokio::runtime::Runtime::new()?;
            let t = Instant::now();
            loop {
                let r = rt.block_on(picyomi_core::cloud::sync_once(&svc))?;
                println!("{} · {:.1}s", serde_json::to_string(&r)?, t.elapsed().as_secs_f64());
                if r.pending_uploads == 0 || args.get(1).map(String::as_str) == Some("--once") {
                    break;
                }
            }
        }
        "highlights" => {
            // highlights: show which part of each card's sentence gets bolded (dry run)
            let db = picyomi_core::Db::open_default()?;
            let rows: Vec<String> = {
                let c = db.lock();
                let mut st = c.prepare("SELECT data FROM cards WHERE deleted=0")?;
                st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?
            };
            for d in rows {
                let card: picyomi_core::cards::Card = serde_json::from_str(&d)?;
                let text: String = if card.sentence_furigana.is_empty() { card.sentence.clone() } else { card.sentence_furigana.iter().map(|s| s.t.as_str()).collect() };
                let show = |h: Option<[usize; 2]>| h.map(|[a, b]| text.chars().skip(a).take(b - a).collect::<String>()).unwrap_or_else(|| "—".into());
                let ruby: String = card
                    .compute_furigana()
                    .iter()
                    .filter(|r| r.r.is_some())
                    .filter(|r| show(card.compute_highlight()).contains(r.t.as_str()))
                    .map(|r| format!("{}【{}】", r.t, r.r.clone().unwrap_or_default()))
                    .collect();
                println!("{}【{}】\t{}\t{}", card.expression, card.reading, show(card.compute_highlight()), ruby);
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
