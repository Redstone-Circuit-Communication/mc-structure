//! Prints a summary of a structure file: `cargo run --example inspect -- door_close.mcstructure`

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: inspect <file>");
        std::process::exit(2);
    };
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("cannot read {path}: {e}");
            std::process::exit(1);
        }
    };
    match mc_structure::parse(&bytes, Some(&path), &mc_structure::Limits::default()) {
        Ok(s) => {
            println!("edition    {} ({}, format_version {})", s.edition.as_str(), s.format.as_str(), s.format_version);
            println!("size       {} × {} × {}", s.size[0], s.size[1], s.size[2]);
            if let Some(o) = s.origin {
                println!("origin     {o:?}");
            }
            println!("blocks     {} (palette {}, block entities {}, entities {})", s.block_count(), s.palette.len(), s.block_entities.len(), s.entities.len());
            let digest: String = s.digest().iter().map(|b| format!("{b:02x}")).collect();
            println!("digest     {digest}");
            println!("materials");
            for (name, n) in s.materials().iter().take(12) {
                println!("  {n:>6}  {name}");
            }
        }
        Err(e) => {
            eprintln!("parse failed: {e}");
            std::process::exit(1);
        }
    }
}
