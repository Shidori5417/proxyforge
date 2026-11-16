mod proxy;

use std::str::FromStr;

fn main() {
    for arg in std::env::args().skip(1) {
        match proxy::Proxy::from_str(&arg) {
            Ok(p) => println!("{}", p),
            Err(err) => eprintln!("{arg}: {err}"),
        }
    }
}
