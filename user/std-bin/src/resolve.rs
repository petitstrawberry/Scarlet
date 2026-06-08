mod resolver_client;

use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let Some(host) = args.next() else {
        println!("Usage: resolve <hostname>");
        return ExitCode::from(1);
    };

    match resolver_client::lookup_ipv4(&host) {
        Ok(addrs) => {
            for addr in addrs {
                println!("{addr}");
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            println!("resolve: {host}: {err}");
            ExitCode::from(1)
        }
    }
}
