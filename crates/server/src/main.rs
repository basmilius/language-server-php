use std::process::ExitCode;

const USAGE: &str = "php-language-server [--stdio]\n\nA PHP language server. It speaks LSP over stdin and stdout.";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        None | Some("--stdio") => {}
        Some("--version" | "-V") => {
            println!("php-language-server {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Some("--help" | "-h") => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Some(other) => {
            eprintln!("unknown argument '{other}'\n\n{USAGE}");
            return ExitCode::from(2);
        }
    }
    let (connection, threads) = lsp_server::Connection::stdio();
    // Parsing recurses, and the default stack of a spawned thread is small.
    let worker = std::thread::Builder::new()
        .name("php-language-server".into())
        .stack_size(64 << 20)
        .spawn(move || php_language_server::run(connection));
    let result = match worker {
        Ok(handle) => handle
            .join()
            .unwrap_or_else(|_| Err("the server thread panicked".into())),
        Err(error) => Err(error.to_string().into()),
    };
    let io_result = threads.join();
    match (result, io_result) {
        (Ok(()), Ok(())) => ExitCode::SUCCESS,
        (Err(error), _) => {
            eprintln!("php-language-server: {error}");
            ExitCode::FAILURE
        }
        (_, Err(error)) => {
            eprintln!("php-language-server: {error}");
            ExitCode::FAILURE
        }
    }
}
