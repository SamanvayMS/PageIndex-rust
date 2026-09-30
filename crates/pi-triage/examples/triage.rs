//! Print triage label counts (and optionally per-page JSON) for PDFs.
//! Usage: triage [--json] <file.pdf>...
fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let json = args.iter().any(|a| a == "--json");
    for path in args.iter().filter(|a| !a.starts_with("--")) {
        let pages = pi_triage::triage_pdf(
            std::path::Path::new(path),
            &pi_triage::Thresholds::default(),
        )?;
        if json {
            println!("{}", serde_json::to_string(&pages)?);
            continue;
        }
        let mut counts = std::collections::BTreeMap::new();
        for p in &pages {
            *counts.entry(format!("{:?}", p.label)).or_insert(0) += 1;
        }
        println!("{path}: {} pages {counts:?}", pages.len());
    }
    Ok(())
}
