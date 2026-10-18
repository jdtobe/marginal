//! Prints the parsed blocks of a Markdown file: `cargo run -p marginal-doc --example dump -- file.md`
fn main() {
    let path = std::env::args().nth(1).expect("usage: dump <file.md>");
    let src = std::fs::read_to_string(path).unwrap();
    let doc = marginal_doc::Document::new(src);
    for b in doc.blocks() {
        println!(
            "{:?} {:?} containers={:?}",
            b.kind,
            b.range,
            b.containers.iter().map(|c| (&c.kind, &doc.text()[c.marker.clone()])).collect::<Vec<_>>()
        );
        for s in &b.segments {
            println!("    {:>4?} {:?} {:?}", s.range, &doc.text()[s.range.clone()], s.kind);
        }
        if let Some(t) = &b.table {
            println!("    aligns {:?}", t.aligns);
            for row in &t.rows {
                println!("    row {:?}", row.iter().map(|r| (r.clone(), &doc.text()[r.clone()])).collect::<Vec<_>>());
            }
        }
        for (i, inl) in b.inlines.iter().enumerate() {
            println!("    inline {i}: {:?} {:?}", inl.kind, inl.range);
        }
    }
}
