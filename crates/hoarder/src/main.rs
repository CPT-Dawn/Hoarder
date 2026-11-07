use clap::Parser;

/// Pixel-exact screenshots and a clipboard that outlives the app that set it.
#[derive(Debug, Parser)]
#[command(name = "hoarder", version, about)]
struct Cli {}

fn main() {
    let _cli = Cli::parse();
}
