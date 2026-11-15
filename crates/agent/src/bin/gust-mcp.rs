//! The same server as `gust --mcp`, without Qt, for the protocol tests.
fn main() {
    std::process::exit(gust_agent::run());
}
