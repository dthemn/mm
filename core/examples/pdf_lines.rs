//! Prints what a PDF is read as, and with LINES=1 its lines, for checking a bank's layout:
//! `cargo run -p mm-core --example pdf_lines -- file.pdf`.
fn main() {
    for path in std::env::args().skip(1) {
        let bytes = std::fs::read(&path).expect("read the file");
        match mm_core::pdf::read(&bytes, std::env::var("PDF_PASSWORD").ok().as_deref()) {
            Ok(pages) => {
                if std::env::var("LINES").is_ok() {
                    for (i, p) in pages.iter().enumerate() {
                        println!("== {path} page {}", i + 1);
                        for l in p {
                            println!("{:6.1} | {}", l.y, l.cells.iter().map(|c| format!("[{:.1}] {}", c.x, c.text)).collect::<Vec<_>>().join("  "));
                        }
                    }
                }
                println!("== {path}: {:?}", mm_core::confirmation::read(&pages));
            }
            Err(e) => println!("== {path}: {e}"),
        }
    }
}
