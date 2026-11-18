//! 看一眼结构文件：`cargo run --example inspect -- door_close.mcstructure`

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("用法：inspect <文件>");
        std::process::exit(2);
    };
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("读不了 {path}：{e}");
            std::process::exit(1);
        }
    };
    match mc_structure::parse(&bytes, Some(&path), &mc_structure::Limits::default()) {
        Ok(s) => {
            println!("版本      {} ({}, format_version {})", s.edition.as_str(), s.format.as_str(), s.format_version);
            println!("尺寸      {} × {} × {}", s.size[0], s.size[1], s.size[2]);
            if let Some(o) = s.origin {
                println!("原点      {o:?}");
            }
            println!("方块      {}（调色板 {} 项，方块实体 {}，实体 {}）", s.block_count(), s.palette.len(), s.block_entities.len(), s.entities.len());
            let digest: String = s.digest().iter().map(|b| format!("{b:02x}")).collect();
            println!("摘要      {digest}");
            println!("材料");
            for (name, n) in s.materials().iter().take(12) {
                println!("  {n:>6}  {name}");
            }
        }
        Err(e) => {
            eprintln!("解析失败：{e}");
            std::process::exit(1);
        }
    }
}
