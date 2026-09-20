mod app;
mod crypto;
mod notes;
mod root;
mod state;
mod sync;

#[tokio::main]
async fn main() {
    if let Err(error) = app::run().await {
        eprintln!("bgh: {error:#}");
        std::process::exit(1);
    }
}
