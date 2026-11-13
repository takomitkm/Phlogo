//! tracing initialization (P7-02): delegates to the observability crate so api and
//! worker share JSON logging + global redaction.
pub fn init(level: &str) {
    // level comes from config verbosity; observability::init honors RUST_LOG first.
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", level);
    }
    observability::init("phlogo-gnosis-curse");
}
