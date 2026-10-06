use std::process::ExitCode;

const USAGE: &str = "php-language-server [--stdio]\n\nA PHP language server. It speaks LSP over stdin and stdout.";

fn main() -> ExitCode {
    lsc_server::run_stdio(
        "php-language-server",
        env!("CARGO_PKG_VERSION"),
        USAGE,
        php_language_server::run,
    )
}
