//! A survey of the whole Typora gallery: download every theme and tally what Marginal can't
//! render, to decide which CSS features are worth supporting. Run on demand only:
//! `MARGINAL_SURVEY_DIR=/tmp/survey cargo test -p marginal-app survey -- --ignored --nocapture`

use std::collections::HashMap;

use super::{download, index};

/// The shape of an unsupported selector, e.g. "descendant (a b)" or ":first-child".
fn selector_kind(sel: &str) -> &'static str {
    if sel.contains("::before") || sel.contains("::after") || sel.contains(":before") || sel.contains(":after") {
        "::before/::after"
    } else if sel.contains(":nth-") {
        ":nth-child/:nth-of-type"
    } else if sel.contains(":first-child") || sel.contains(":last-child") || sel.contains(":first-of-type") {
        ":first-child/:last-child"
    } else if sel.contains(":not(") {
        ":not()"
    } else if sel.contains('[') {
        "attribute [..]"
    } else if sel.contains(" + ") || sel.contains(" ~ ") {
        "sibling (+ ~)"
    } else if sel.contains('>') {
        "child (a > b)"
    } else if sel.contains(':') {
        "other pseudo-class"
    } else if sel.split_whitespace().count() > 1 {
        "descendant (a b)"
    } else if sel.contains('#') {
        "id"
    } else {
        "other"
    }
}

#[test]
#[ignore = "downloads the whole gallery"]
fn survey() {
    let dir = std::path::PathBuf::from(std::env::var("MARGINAL_SURVEY_DIR").expect("set MARGINAL_SURVEY_DIR"));
    // Reuse the listing between runs: GitHub allows 60 anonymous API calls an hour.
    let cached = dir.join("index.json");
    let entries = match index::Cached::load(&cached) {
        Some(c) => c.entries,
        None => {
            let entries = index::fetch().unwrap();
            index::Cached { fetched_at: index::now_secs(), entries: entries.clone() }.save(&cached).unwrap();
            entries
        }
    };
    let chunks: Vec<Vec<index::Entry>> = entries.chunks(entries.len().div_ceil(8)).map(<[_]>::to_vec).collect();
    let results: Vec<(String, Result<marginal_theme::Theme, String>)> = std::thread::scope(|s| {
        let workers: Vec<_> = chunks
            .into_iter()
            .map(|chunk| {
                let dir = dir.clone();
                s.spawn(move || {
                    chunk
                        .into_iter()
                        .map(|e| {
                            let target = dir.join(&e.slug);
                            let variants = match target.exists() {
                                true => Ok(download::variants(&target)),
                                false => download::download(e.download.as_deref(), e.homepage.as_deref(), &target),
                            };
                            let theme = variants.and_then(|v| {
                                let v =
                                    download::matching(&v, &e.title).into_iter().next().ok_or("no css".to_owned())?;
                                marginal_theme::Theme::load_file(&v.css)
                            });
                            (e.title.clone(), theme)
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        workers.into_iter().flat_map(|w| w.join().unwrap()).collect()
    });

    let mut failed = Vec::new();
    let mut ratios = Vec::new();
    let mut features: HashMap<String, (usize, usize)> = HashMap::new(); // (themes, declarations)
    let mut selectors: HashMap<&str, (usize, usize)> = HashMap::new();
    let mut props: HashMap<String, (usize, usize)> = HashMap::new();
    for (title, theme) in &results {
        let Ok(theme) = theme else {
            failed.push((title, theme.as_ref().err().unwrap()));
            continue;
        };
        ratios.push((theme.fit.ratio(), title));
        for (f, n) in &theme.fit.missing {
            let e = features.entry(f.clone()).or_default();
            e.0 += 1;
            e.1 += n;
        }
        let mut seen_sel: Vec<&str> = Vec::new();
        let mut seen_prop: Vec<String> = Vec::new();
        for d in &theme.diagnostics {
            if let Some(sel) = d.message.strip_prefix("unsupported selector `").and_then(|s| s.strip_suffix('`')) {
                if marginal_theme::is_app_chrome(sel) {
                    continue;
                }
                let k = selector_kind(sel);
                let e = selectors.entry(k).or_default();
                e.1 += 1;
                if !seen_sel.contains(&k) {
                    seen_sel.push(k);
                    e.0 += 1;
                }
            } else if let Some(p) = d.message.strip_prefix("unsupported property `").and_then(|s| s.strip_suffix('`')) {
                let e = props.entry(p.to_owned()).or_default();
                e.1 += 1;
                if !seen_prop.contains(&p.to_owned()) {
                    seen_prop.push(p.to_owned());
                    e.0 += 1;
                }
            }
        }
    }
    ratios.sort_by(|a, b| a.0.total_cmp(&b.0));
    let n = ratios.len();
    let good = ratios.iter().filter(|r| r.0 >= 0.8).count();
    let partial = ratios.iter().filter(|r| r.0 >= 0.5 && r.0 < 0.8).count();
    println!(
        "\n{} themes parsed, {} failed. Good {good}, Partial {partial}, Poor {}",
        n,
        failed.len(),
        n - good - partial
    );
    println!("median fit {:.0}%", ratios.get(n / 2).map_or(0.0, |r| r.0 * 100.0));
    let top = |m: Vec<(String, (usize, usize))>, label: &str| {
        let mut m = m;
        m.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(b.1.1.cmp(&a.1.1)));
        println!("\n{label} (themes, declarations):");
        for (k, (t, d)) in m.into_iter().take(30) {
            println!("  {t:>4} {d:>6}  {k}");
        }
    };
    top(features.into_iter().collect(), "Missing features");
    top(selectors.into_iter().map(|(k, v)| (k.to_owned(), v)).collect(), "Unsupported selector shapes");
    top(props.into_iter().collect(), "Unsupported properties");
    println!("\nFailures:");
    for (t, e) in failed.iter().take(15) {
        println!("  {t}: {e}");
    }
}
