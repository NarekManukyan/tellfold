fn main() {
    let mut argv = std::env::args();
    let argv0 = argv.next().unwrap_or_default();
    let args: Vec<String> = argv.collect();
    std::process::exit(tellfold_cli::app::main(&argv0, &args));
}
