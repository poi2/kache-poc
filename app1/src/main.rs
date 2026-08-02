use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Msg {
    text: String,
}

fn main() -> anyhow::Result<()> {
    let m = Msg { text: "hello from app1".into() };
    println!("{}", m.text);
    Ok(())
}
