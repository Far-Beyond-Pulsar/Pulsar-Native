//! Places the helper beside this executable and says what happened: run it twice.
fn main() {
    println!("embedded: {}", pulsar_dialog_embed::is_embedded());
    println!("{:?}", pulsar_dialog_embed::ensure_helper());
}
