//! Does the vocabulary list actually earn its place?
//!
//! Run explicitly:
//!   cargo test --release --test vocabulary -- --nocapture --ignored

use flow::refine::{Cleanup, Style};

const MANGLED: &[&str] = &[
    // What Parakeet actually produced when the speaker said "Flow".
    "so this is a test recording from the application film",
    "I pushed the change to the hyper land config",
    "open the file in nvm and check the pipe wire logs",
];

#[test]
#[ignore]
fn compare_with_and_without_vocabulary() {
    let path = flow::refine::model_path();
    if !path.is_file() {
        eprintln!("skipping: no refining model");
        return;
    }

    // One load for both halves of the comparison. Two concurrent loads on the
    // same Vulkan device segfault, and the vocabulary is a property of the
    // prompt rather than of the model, so there was never a reason for the
    // list to be baked into the weights being loaded.
    let refiner = flow::refine::Refiner::load(&path, None).expect("load");
    let bare = Style::new(Cleanup::Light);
    let informed = bare.clone().with_vocabulary(
        ["Flow", "Hyprland", "Neovim", "PipeWire"]
            .map(str::to_string)
            .to_vec(),
    );

    for raw in MANGLED {
        let refine = |style: &Style| {
            refiner
                .refine_within(raw, std::time::Duration::from_secs(120), style)
                .expect("refine")
        };
        eprintln!("\nraw:        {raw:?}");
        eprintln!("no vocab:   {:?}", refine(&bare));
        eprintln!("with vocab: {:?}", refine(&informed));
    }
}
