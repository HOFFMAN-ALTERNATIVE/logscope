use clap::Parser;

fn main() {
    let cli = logscope::Cli::parse();
    match logscope::run(cli) {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("logscope: error: {e:#}");
            std::process::exit(2);
        }
    }
}
